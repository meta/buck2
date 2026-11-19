/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

mod fmt_suppression;
mod load_utils;
mod move_loads_to_top;
pub mod parsed_module;
mod sort_dict_keys;
mod sort_kwargs;
mod sort_list_args;
mod sort_loads;
mod unused_loads;
mod utils;

use std::borrow::Cow;
use std::path::Path;

use parsed_module::ParsedModule;
use tracing::info_span;

use crate::config::Config;
use crate::config::SortFlags;

/// Input to the autofix pipeline: source text and the file path, which is
/// matched against config overrides to decide which passes to run.
pub(crate) struct AutofixInput<'a> {
    pub(crate) source: &'a str,
    pub(crate) path: &'a Path,
}

pub(crate) fn apply_autofixes(input: &AutofixInput<'_>, config: &Config) -> anyhow::Result<String> {
    let source = input.source;
    if source.is_empty() {
        return Ok(String::new());
    }

    let SortFlags {
        list_args: sort_rule_args,
        kwargs: sort_kwargs,
    } = config.sort_flags(input.path);

    let module = info_span!("initial_parse")
        .in_scope(|| ParsedModule::parse_with_path(Cow::Borrowed(source), Some(input.path)))?;
    let module = module
        .run_transform(info_span!("unused_loads").in_scope(|| unused_loads::collect_edits))?;
    let module = module.run_transform(
        info_span!("move_loads_to_top").in_scope(|| move_loads_to_top::collect_edits),
    )?;
    let module =
        module.run_transform(info_span!("sort_loads").in_scope(|| sort_loads::collect_edits))?;

    // Sort dict keys - run multiple passes to handle nested dicts
    let mut module = module
        .run_transform(info_span!("sort_dict_keys").in_scope(|| sort_dict_keys::collect_edits))?;
    for _ in 0..5 {
        let (next, changed) = module.run_transform_checked(
            info_span!("sort_dict_keys").in_scope(|| sort_dict_keys::collect_edits),
        )?;
        module = next;
        if !changed {
            break;
        }
    }

    // Sort list arguments (deps, srcs, visibility, etc.). `sort_rule_args`
    // gates sorting of allowlisted rule args (disabled e.g. for .bzl files via
    // config); explicit `# keep sorted` lists are sorted either way.
    let module = info_span!("sort_list_args")
        .in_scope(|| sort_list_args::apply(module, config, sort_rule_args))
        .map_err(|error| anyhow::anyhow!("{}:{error:#}", input.path.display()))?;

    // Sort keyword arguments by priority - run multiple passes to handle nested calls.
    // When an outer call reorders kwargs containing inner calls, the outer edit
    // overwrites inner edits (they target overlapping ranges). Subsequent passes
    // sort the inner calls once the outer call is stable.
    // Buildifier scopes sortCallArgs to BUILD/WORKSPACE/MODULE files only, so
    // config disables this pass e.g. for .bzl files.
    let mut module = module;
    if sort_kwargs {
        module = module.run_transform(
            info_span!("sort_kwargs")
                .in_scope(|| |m: &ParsedModule| sort_kwargs::collect_edits(m, config)),
        )?;
        for _ in 0..5 {
            let (next, changed) = module.run_transform_checked(
                info_span!("sort_kwargs")
                    .in_scope(|| |m: &ParsedModule| sort_kwargs::collect_edits(m, config)),
            )?;
            module = next;
            if !changed {
                break;
            }
        }
    }

    Ok(module.unparse())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Config with a `.bzl` override matching production: rule-arg and kwarg
    /// sorting are disabled for `.bzl`, enabled for BUILD-like files.
    fn test_config() -> Config {
        serde_json::from_str(
            r#"{
                "IsSortableListArg": {"deps": true, "srcs": true, "visibility": true},
                "SortableBlacklist": {},
                "NamePriority": {"name": -99, "deps": 10, "visibility": 90},
                "Overrides": [
                    {"Files": ["*.bzl"], "SortListArgs": false, "SortKwargs": false}
                ]
            }"#,
        )
        .expect("valid test config")
    }

    fn bzl_input(source: &str) -> AutofixInput<'_> {
        AutofixInput {
            source,
            path: Path::new("test.bzl"),
        }
    }

    fn build_input(source: &str) -> AutofixInput<'_> {
        AutofixInput {
            source,
            path: Path::new("TARGETS"),
        }
    }

    #[test]
    fn test_sort_kwargs_skipped_for_bzl() {
        let source = r#"my_rule(deps = [], name = "x", visibility = [])
"#;
        let config = test_config();
        let result = apply_autofixes(&bzl_input(source), &config).unwrap();
        assert_eq!(result, source, "kwargs should not be sorted in .bzl files");
    }

    #[test]
    fn test_sort_kwargs_applied_for_build() {
        let source = r#"my_rule(deps = [], name = "x", visibility = [])
"#;
        let config = test_config();
        let result = apply_autofixes(&build_input(source), &config).unwrap();
        assert_ne!(result, source, "kwargs should be sorted in BUILD files");
        assert!(
            result.find("name").unwrap() < result.find("deps").unwrap(),
            "name should come before deps in BUILD files"
        );
    }

    #[test]
    fn test_sort_list_args_skipped_for_bzl() {
        let source = r#"my_rule(name = "x", deps = [":z", ":a", ":m"])
"#;
        let config = test_config();
        let result = apply_autofixes(&bzl_input(source), &config).unwrap();
        assert!(
            result.contains(r#"[":z", ":a", ":m"]"#),
            "list args should not be sorted in .bzl files, got: {result}"
        );
    }

    #[test]
    fn test_sort_list_args_applied_for_build() {
        let source = r#"my_rule(name = "x", deps = [":z", ":a", ":m"])
"#;
        let config = test_config();
        let result = apply_autofixes(&build_input(source), &config).unwrap();
        assert!(
            result.contains(r#"[":a", ":m", ":z"]"#),
            "list args should be sorted in BUILD files, got: {result}"
        );
    }

    #[test]
    fn test_sort_loads_still_runs_for_bzl() {
        let source = r#"load("//z:z.bzl", "z_func")
load("//a:a.bzl", "a_func")

x = z_func() + a_func()
"#;
        let config = test_config();
        let result = apply_autofixes(&bzl_input(source), &config).unwrap();
        assert!(
            result.find("//a:a.bzl").unwrap() < result.find("//z:z.bzl").unwrap(),
            "loads should still be sorted in .bzl files, got: {result}"
        );
    }

    #[test]
    fn test_sort_dict_keys_still_runs_for_bzl() {
        let source = r#"d = {"z": 1, "a": 2}
"#;
        let config = test_config();
        let result = apply_autofixes(&bzl_input(source), &config).unwrap();
        assert!(
            result.find(r#""a""#).unwrap() < result.find(r#""z""#).unwrap(),
            "dict keys should still be sorted in .bzl files, got: {result}"
        );
    }

    #[test]
    fn test_select_comma_less_none_branch_through_full_pipeline() {
        // The reported BUCKFORMAT crash exercised several passes at once: a
        // `select()` whose dict keys must be reordered, whose list value must be
        // sorted, and whose relocated final branch (`"DEFAULT": None`) omits its
        // trailing comma. Running the whole pipeline must reorder everything AND
        // insert the comma so the output re-parses, rather than crashing.
        let source = indoc::indoc! {r#"
            my_rule(
                deps = select({
                    "ovr_config//cpu:arm64": [":z", ":a"],
                    "DEFAULT": None
                }),
                name = "x",
            )
        "#};
        let expected = indoc::indoc! {r#"
            my_rule(
                name = "x",
                deps = select({
                    "DEFAULT": None,
                    "ovr_config//cpu:arm64": [":a", ":z"],
                }),
            )
        "#};
        let config = test_config();
        let result = apply_autofixes(&build_input(source), &config).expect("must not crash");
        assert_eq!(result, expected);
        // Idempotent: a second run over the fixed output is a no-op.
        let again = apply_autofixes(&build_input(&result), &config).expect("must not crash");
        assert_eq!(again, expected);
    }

    #[test]
    fn test_inline_sort_error_includes_path() {
        let source = indoc::indoc! {r#"
            my_rule(
                # starlark-fmt: sort-by = "string"
                custom_items = [some_call()],
            )
        "#};
        let config = test_config();

        let error = apply_autofixes(
            &AutofixInput {
                source,
                path: Path::new("foo/TARGETS"),
            },
            &config,
        )
        .expect_err("unmatched inline sort key should fail");

        assert!(
            format!("{error:#}").contains("foo/TARGETS:3:21-32"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn test_fmt_off_ignores_inline_sort_directive() {
        let source = indoc::indoc! {r#"
            # fmt: off
            my_rule(
                # starlark-fmt: sort-by = {"unknown": true}
                custom_items = [zebra(), alpha()],
            )
            # fmt: on
        "#};
        let config = test_config();

        let result = apply_autofixes(
            &AutofixInput {
                source,
                path: Path::new("foo/TARGETS"),
            },
            &config,
        )
        .expect("fmt-off directive should be ignored");

        assert_eq!(result, source);
    }

    #[test]
    fn test_inline_sort_with_nested_sortable_lists_is_idempotent() {
        let source = indoc::indoc! {r#"
            my_rule(
                # starlark-fmt: sort-by = {"call_keyword": "name"}
                custom_items = [
                    foo(name = "z", deps = [":z", ":a"]),
                    foo(name = "a", deps = [":z", ":a"]),
                ],
            )
        "#};
        let expected = indoc::indoc! {r#"
            my_rule(
                # starlark-fmt: sort-by = {"call_keyword": "name"}
                custom_items = [
                    foo(name = "a", deps = [":a", ":z"]),
                    foo(name = "z", deps = [":a", ":z"]),
                ],
            )
        "#};
        let config = test_config();

        let once = apply_autofixes(&build_input(source), &config).expect("first pass should work");
        let twice = apply_autofixes(&build_input(&once), &config).expect("second pass should work");

        assert_eq!(once, expected);
        assert_eq!(twice, expected);
    }

    #[test]
    fn test_inline_sort_defers_length_changing_nested_edits() {
        let source = indoc::indoc! {r#"
            my_rule(
                # starlark-fmt: sort-by = {"call_keyword": "name"}
                custom_items = [
                    foo(name = "z", deps = [":a", ":a"]),
                    foo(name = "a", deps = [":b", ":b"]),
                ],
                visibility = ["PUBLIC"],
            )
        "#};
        let expected = indoc::indoc! {r#"
            my_rule(
                # starlark-fmt: sort-by = {"call_keyword": "name"}
                custom_items = [
                    foo(name = "a", deps = [":b"]),
                    foo(name = "z", deps = [":a"]),
                ],
                visibility = ["PUBLIC"],
            )
        "#};
        let config = test_config();

        let result = apply_autofixes(&build_input(source), &config)
            .expect("overlapping edits should be applied in separate passes");

        assert_eq!(result, expected);
    }

    #[test]
    fn test_deeply_nested_inline_sorts_reach_a_fixed_point() {
        fn nested_call(depth: usize) -> String {
            if depth == 0 {
                return "leaf()".to_owned();
            }
            format!(
                "wrapper(\n# starlark-fmt: sort-by = \"call_name\"\nitems = [z(child = {}), a()],\n)",
                nested_call(depth - 1)
            )
        }

        let source = format!("root(value = {})\n", nested_call(7));
        let config = test_config();

        let once =
            apply_autofixes(&build_input(&source), &config).expect("first pass should converge");
        let twice =
            apply_autofixes(&build_input(&once), &config).expect("second pass should converge");

        assert_eq!(
            once.matches("items = [a(), z(").count(),
            7,
            "every nested custom list should be sorted in the first pass: {once}"
        );
        assert_eq!(once, twice);
    }
}
