/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

mod collapse_single_literal_call;
mod join_plus_call;
mod kwarg_spaces;
mod subscript_commas;

use std::borrow::Cow;
use std::path::Path;

use ruff_formatter::LineWidth;
use ruff_python_formatter::MagicTrailingComma;
use ruff_python_formatter::PyFormatOptions;
use ruff_python_formatter::format_module_source;
use tracing::info_span;

use crate::autofixes::parsed_module::ParsedModule;

/// Line width to match buildifier default (160 characters).
const LINE_WIDTH: u16 = 160;
const JOINABLE_PLUS_CALLS: &[&str] = &["select"];

#[cfg(test)]
pub fn format_file(source: String) -> anyhow::Result<String> {
    format_file_with_path(source, None)
}

pub fn format_file_with_path(source: String, path: Option<&Path>) -> anyhow::Result<String> {
    let formatted = info_span!("run_ruff_formatter")
        .in_scope(|| {
            format_module_source(
                &source,
                // Match buildifier heuristics:
                //   - 4-space indent (ruff default)
                //   - Double quotes (ruff default)
                //   - 160-char line width (buildifier default)
                //   - Respect magic trailing comma: a trailing comma signals "keep multi-line"
                PyFormatOptions::default()
                    .with_line_width(
                        LineWidth::try_from(LINE_WIDTH).expect("LINE_WIDTH should be valid"),
                    )
                    .with_magic_trailing_comma(MagicTrailingComma::Respect),
            )
        })
        .map_err(|err| match path {
            Some(p) => anyhow::anyhow!("{}: {}", p.display(), err),
            None => err.into(),
        })?;

    let code = info_span!("ruff_formatter_to_text").in_scope(|| formatted.into_code());

    let kwarg_fixed = info_span!("post_process_formatting").in_scope(|| {
        ParsedModule::parse_with_path(Cow::Owned(code), path)
            .and_then(|module| {
                module.run_transform(|module| {
                    info_span!("kwarg_spaces").in_scope(|| kwarg_spaces::collect_edits(module))
                })
            })
            .and_then(|module| {
                // Remove trailing commas from subscript expressions (e.g., dict[str, int,]).
                // Go-based Starlark interpreters don't support trailing commas in this context.
                module.run_transform(|module| {
                    info_span!("subscript_commas")
                        .in_scope(|| subscript_commas::collect_edits(module))
                })
            })
            .and_then(|module| {
                let mut module = module;
                for _ in 0..5 {
                    let (next, changed) = module.run_transform_checked(|module| {
                        info_span!("collapse_single_literal_call")
                            .in_scope(|| collapse_single_literal_call::collect_edits(module))
                    })?;
                    module = next;
                    if !changed {
                        break;
                    }
                }
                Ok(module)
            })
            .and_then(|module| {
                module.run_transform(|module| {
                    info_span!("join_plus_call")
                        .in_scope(|| join_plus_call::collect_edits(module, JOINABLE_PLUS_CALLS))
                })
            })
            .map(|module| module.unparse())
    })?;

    // Collapse runs of >2 consecutive newlines to exactly 2 (max one blank
    // line). Ruff can expand blank lines inside `# fmt: off` blocks on each
    // pass, making formatting non-idempotent. This normalization fixes that.
    Ok(info_span!("normalize_blank_lines").in_scope(|| collapse_blank_lines(kwarg_fixed)))
}

/// Replace runs of 3+ consecutive newlines with exactly 2 (`\n\n`),
/// preserving at most one blank line between any two non-empty lines.
fn collapse_blank_lines(source: String) -> String {
    if !source
        .as_bytes()
        .windows(3)
        .any(|window| window == b"\n\n\n")
    {
        return source;
    }

    let mut result = String::with_capacity(source.len());
    let mut newline_count = 0u32;

    for ch in source.chars() {
        if ch == '\n' {
            newline_count += 1;
            if newline_count <= 2 {
                result.push(ch);
            }
        } else {
            newline_count = 0;
            result.push(ch);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::*;

    #[test]
    fn test_idempotent() {
        let source = indoc! {r#"
            load("//core:defs.bzl", "rule_a")
            rule_a(name = "foo")
        "#};
        let first = format_file(source.to_owned()).expect("first format failed");
        let second = format_file(first.clone()).expect("second format failed");
        assert_eq!(first, second, "formatting should be idempotent");
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(format_file(String::new()).expect("format failed"), "");
    }

    #[test]
    fn test_preserves_comments() {
        let source = indoc! {r#"
            # Top comment
            load("//foo:bar.bzl", "rule")  # inline
        "#};
        let formatted = format_file(source.to_owned()).expect("format failed");
        assert!(formatted.contains("# Top comment"), "top comment preserved");
        assert!(formatted.contains("# inline"), "inline comment preserved");
    }

    #[test]
    fn test_adds_spacing() {
        let source = "x=1+2\n";
        let formatted = format_file(source.to_owned()).expect("format failed");
        assert!(
            formatted.contains("x = 1 + 2"),
            "should add spacing around operators"
        );
    }

    #[test]
    fn test_respects_magic_trailing_comma() {
        // Trailing comma signals "keep multi-line"
        let source = indoc! {r#"
            x = [
                1,
                2,
                3,
            ]
        "#};
        let formatted = format_file(source.to_owned()).expect("format failed");
        assert!(
            formatted.contains('\n'),
            "should keep multi-line when trailing comma present"
        );
    }

    #[test]
    fn test_collapses_without_trailing_comma() {
        // No trailing comma = formatter can collapse
        let source = indoc! {r#"
            x = [
                1,
                2,
                3
            ]
        "#};
        let formatted = format_file(source.to_owned()).expect("format failed");
        assert_eq!(
            formatted.trim(),
            "x = [1, 2, 3]",
            "should collapse short list without trailing comma"
        );
    }

    #[test]
    fn test_double_quotes_preserved() {
        let source = "load(\"//foo:bar.bzl\", \"rule\")\n";
        let formatted = format_file(source.to_owned()).expect("format failed");
        assert!(formatted.contains('"'), "double quotes should be preserved");
    }

    #[test]
    fn test_collapse_blank_lines() {
        assert_eq!(collapse_blank_lines("a\n\n\nb".to_owned()), "a\n\nb");
        assert_eq!(collapse_blank_lines("a\n\n\n\n\nb".to_owned()), "a\n\nb");
        assert_eq!(collapse_blank_lines("a\n\nb".to_owned()), "a\n\nb");
        assert_eq!(collapse_blank_lines("a\nb".to_owned()), "a\nb");
        assert_eq!(collapse_blank_lines(String::new()), "");
        assert_eq!(collapse_blank_lines("\n\n\n".to_owned()), "\n\n");
    }

    #[test]
    fn test_fmt_off_on_idempotent() {
        // Ruff expands blank lines inside `# fmt: off` blocks on each pass.
        // This test verifies our collapse_blank_lines fix.
        let source = indoc! {r#"
            oncall('x')

            # FIXME: broken

            # fmt: off

            # fmt: on

        "#};
        let first = format_file(source.to_owned()).expect("first format failed");
        let second = format_file(first.clone()).expect("second format failed");
        assert_eq!(first, second, "fmt: off/on should be idempotent");
    }

    #[test]
    fn test_escaped_string_continuation_idempotent() {
        let source = indoc! {r#"
            def _multi_mcu_build(mcu_list, package):
                bash_cmd = " ; ".join(["mkdir -p ${OUT}/" + mcu + " ; \
                    cp -R $(location //" + package + "/" + mcu + ":artifacts)/. \
                    ${OUT}/" + mcu + "/" for mcu in mcu_list])
        "#};

        let first = format_file(source.to_owned()).expect("first format failed");
        let second = format_file(first.clone()).expect("second format failed");
        assert_eq!(
            first, second,
            "escaped string continuations should be stable after one formatter invocation"
        );
    }

    #[test]
    fn test_triple_quoted_string_in_join_idempotent() {
        let source = indoc! {r###"
            def module_name_consistency_test(name, module):
                bash = """
                    echo '{}' >> $OUT
                    """.format(
                    "\n".join([
                        """TEST({0}, {1}ModuleName) {{
                            EXPECT_EQ(
                                actual,
                                expected);
                        }}""".format(name, module),
                    ]),
                )
        "###};

        let first = format_file(source.to_owned()).expect("first format failed");
        let second = format_file(first.clone()).expect("second format failed");
        assert_eq!(
            first, second,
            "triple-quoted string literals inside join calls should be stable"
        );
    }

    #[test]
    fn test_four_space_indent() {
        let source = indoc! {r#"
            if True:
              x = 1
        "#};
        let formatted = format_file(source.to_owned()).expect("format failed");
        assert!(formatted.contains("    x = 1"), "should use 4-space indent");
    }

    #[test]
    fn test_long_load_wraps_at_line_width() {
        // A load() statement that exceeds 160 chars should be wrapped to multiple lines
        let source = r#"load("//very/long/path/to/some/deeply/nested/package:defs.bzl", "rule_alpha", "rule_beta", "rule_gamma", "rule_delta", "rule_epsilon", "rule_zeta", "rule_eta", "rule_theta")"#;
        assert!(
            source.len() > LINE_WIDTH as usize,
            "test input should exceed {} chars, got {}",
            LINE_WIDTH,
            source.len()
        );

        let formatted = format_file(source.to_owned()).expect("format failed");

        let expected = indoc! {r#"
            load(
                "//very/long/path/to/some/deeply/nested/package:defs.bzl",
                "rule_alpha",
                "rule_beta",
                "rule_gamma",
                "rule_delta",
                "rule_epsilon",
                "rule_zeta",
                "rule_eta",
                "rule_theta",
            )
        "#};
        assert_eq!(
            formatted, expected,
            "long load() should be wrapped with each symbol on its own line"
        );
    }

    #[test]
    fn test_collapses_any_single_literal_call() {
        let source = indoc! {r#"
            x = some_func(
                {
                    "DEFAULT": [],
                }
            )
        "#};
        let expected = indoc! {r#"
            x = some_func({
                "DEFAULT": [],
            })
        "#};
        let formatted = format_file(source.to_owned()).expect("format failed");
        assert_eq!(
            formatted, expected,
            "some_func() with a single dict argument should collapse around the dict"
        );
    }

    #[test]
    fn test_collapses_join_list_comprehension_call() {
        let source = indoc! {r#"
            merge_pairs = " ".join([
                    "$(location :{TARGET}) {DEST}".format(
                        TARGET = _archive_target_name(n, k),
                        DEST = dest,
                    )
                    for (n, k, dest) in contributing
                ])
        "#};
        let expected = indoc! {r#"
            merge_pairs = " ".join([
                "$(location :{TARGET}) {DEST}".format(
                    TARGET = _archive_target_name(n, k),
                    DEST = dest,
                )
                for (n, k, dest) in contributing
            ])
        "#};
        let formatted = format_file(source.to_owned()).expect("format failed");
        assert_eq!(
            formatted, expected,
            "join() with a single list comprehension argument should collapse around the list"
        );
    }

    #[test]
    fn test_joins_select_call_after_plus() {
        let source = indoc! {r#"
            x = (
                ["A"]
                + select({"DEFAULT": []})
            )
        "#};
        let formatted = format_file(source.to_owned()).expect("format failed");
        assert_eq!(formatted, "x = [\"A\"] + select({\"DEFAULT\": []})\n");
    }

    #[test]
    fn test_collapses_nested_select_single_dict_calls() {
        let source = indoc! {r#"
            x = select(
                {
                    "DEFAULT": select(
                        {
                            "DEFAULT": [],
                        }
                    ),
                }
            )
        "#};
        let expected = indoc! {r#"
            x = select({
                "DEFAULT": select({
                    "DEFAULT": [],
                }),
            })
        "#};
        let formatted = format_file(source.to_owned()).expect("format failed");
        assert_eq!(
            formatted, expected,
            "nested select() calls should collapse without overlapping edits"
        );
    }
}
