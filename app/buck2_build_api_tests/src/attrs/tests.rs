/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_analysis::attrs::resolve::configured_attr::ConfiguredAttrExt;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use buck2_build_api::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;
use buck2_build_api::interpreter::rule_defs::provider::registration::register_builtin_providers;
use buck2_common::package_listing::listing::PackageListing;
use buck2_common::package_listing::listing::testing::PackageListingExt;
use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_root_path::CellRootPathBuf;
use buck2_core::cells::name::CellName;
use buck2_core::configuration::compatibility::IncompatiblePlatformReasonCause;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::execution_types::executor_config::PathSeparatorKind;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::buck_out_path::BuckOutPathResolver;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::package::PackageLabel;
use buck2_core::plugins::PluginKindSet;
use buck2_error::ErrorTag;
use buck2_execute::artifact::fs::ExecutorFs;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_hash::BuckMutMap;
use buck2_interpreter_for_build::attrs::coerce::testing::CoerceSealed;
use buck2_interpreter_for_build::attrs::coerce::testing::coercion_ctx;
use buck2_interpreter_for_build::attrs::coerce::testing::coercion_ctx_listing;
use buck2_interpreter_for_build::attrs::coerce::testing::to_value;
use buck2_interpreter_for_build::interpreter::selector::register_select;
use buck2_node::attrs::arena::OwnedCoercedAttr;
use buck2_node::attrs::attr::Attribute;
use buck2_node::attrs::attr_type::AttrType;
use buck2_node::attrs::coerced_attr_full::CoercedAttrFull;
use buck2_node::attrs::coerced_deps_collector::CoercedDepsCollector;
use buck2_node::attrs::configuration_context::AttrConfigurationContext;
use buck2_node::attrs::configured_attr_info_for_tests::ConfiguredAttrInfoForTests;
use buck2_node::attrs::display::AttrDisplayWithContextExt;
use buck2_node::attrs::fmt_context::AttrFmtContext;
use buck2_node::attrs::testing::configuration_ctx;
use buck2_node::attrs::testing::configuration_ctx_unmatched_select_incompatible;
use buck2_node::provider_id_set::ProviderIdSet;
use buck2_util::arc_str::ArcStr;
use dupe::Dupe;
use indoc::indoc;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Module;
use starlark::values::Heap;
use starlark::values::UnpackValue;
use starlark::values::Value;

use crate::attrs::resolve::testing::resolution_ctx;
use crate::attrs::resolve::testing::resolution_ctx_with_providers;

#[test]
fn test() -> buck2_error::Result<()> {
    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard().with(register_select).build();

        // Check that `x` is captured with the function
        let value = to_value(
            &env,
            &globals,
            indoc!(
                r#"
                [[
                    ["hello", "world!"]
                    + select({
                        "//some:config": ["some"],
                        "DEFAULT": ["okay"] + select({
                            "//other:config": ["other"],
                            "DEFAULT": ["default", "for", "realz"],
                        }),
                    })
                    + ["..."]
                    + ["..."]
                ]]
                "#
            ),
        );

        let attr = AttrType::list(AttrType::list(AttrType::list(AttrType::string())));

        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        assert_eq!(
            "[[[\"hello\", \"world!\"]+select({\"root//some:config\": [\"some\"], \"DEFAULT\": [\"okay\"]+select({\"root//other:config\": [\"other\"], \"DEFAULT\": [\"default\", \"for\", \"realz\"]})})+[\"...\"]+[\"...\"]]]",
            coerced.as_ref().as_display_no_ctx().to_string()
        );

        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!(
            "[[[\"hello\", \"world!\", \"okay\", \"other\", \"...\", \"...\"]]]",
            configured.as_display_no_ctx().to_string()
        );

        let mut ctx = resolution_ctx(&env);
        let resolved = configured.resolve_single(PackageLabel::testing(), &mut ctx)?;
        assert_eq!(
            "[[[\"hello\", \"world!\", \"okay\", \"other\", \"...\", \"...\"]]]",
            resolved.to_string()
        );

        Ok(())
    })
}

#[test]
fn test_string() -> buck2_error::Result<()> {
    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard().with(register_select).build();
        let attr = AttrType::string();
        let value = to_value(&env, &globals, r#""a" + select({"DEFAULT": "b"})"#);

        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!(r#""ab""#, configured.as_display_no_ctx().to_string());

        Ok(())
    })
}

#[test]
fn test_unmatched_select() -> buck2_error::Result<()> {
    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard().with(register_select).build();
        let attr = AttrType::string();
        let value = to_value(&env, &globals, r#""a" + select({"//some:config": "b"})"#);
        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;

        let err = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), Some("srcs"))
            .require_compatible()
            .expect_err("unmatched select without a default should fail");
        assert!(
            err.to_string()
                .contains("None of 1 conditions matched configuration"),
            "err: {err}"
        );

        match coerced
            .as_ref()
            .configure(
                &attr,
                &configuration_ctx_unmatched_select_incompatible("use the fan-out alias"),
                Some("srcs"),
            )
            .ok()?
        {
            MaybeCompatible::Incompatible(reason) => {
                assert_eq!(
                    reason.cause,
                    IncompatiblePlatformReasonCause::UnmatchedSelect {
                        attr: Some(ArcStr::from("srcs")),
                        reason: ArcStr::from("use the fan-out alias"),
                    }
                );
            }
            MaybeCompatible::Compatible(configured) => {
                panic!(
                    "expected incompatible, got {}",
                    configured.as_display_no_ctx()
                )
            }
        }

        Ok(())
    })
}

#[test]
fn test_invalid_concat_coercion_into_one_of() -> buck2_error::Result<()> {
    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard().with(register_select).build();

        let value = to_value(
            &env,
            &globals,
            indoc!(
                r#"
            [True] + select({"DEFAULT": ["foo"]})
            "#
            ),
        );
        let attr = AttrType::one_of(vec![
            AttrType::list(AttrType::bool()),
            AttrType::list(AttrType::string()),
        ]);

        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        let err = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()
            .expect_err("Should fail to concatenate configured lists");
        assert!(
            err.to_string()
                .contains("Cannot concatenate values coerced/configured to different oneof variants: `attrs.list(attrs.bool())` and `attrs.list(attrs.string())`"),
            "err: {err}"
        );
        Ok(())
    })
}

#[test]
fn test_configured_dep_traversal_rejects_duplicate_dict_concat_keys() -> buck2_error::Result<()> {
    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard().with(register_select).build();

        let value = to_value(
            &env,
            &globals,
            indoc!(
                r#"
            {"a": 1} + select({"DEFAULT": {"a": 2}})
            "#
            ),
        );
        let attr = AttrType::dict(AttrType::string(), AttrType::int(), false);

        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        let mut info = ConfiguredAttrInfoForTests::new();
        let err = coerced
            .as_ref()
            .traverse_configured(
                &attr,
                PackageLabel::testing(),
                &configuration_ctx(),
                &mut info,
                None,
            )
            .require_compatible()
            .expect_err("a key on both sides of the concat must fail dep collection");
        assert!(
            err.to_string()
                .contains("got same key in both sides of dictionary concat"),
            "err: {err}"
        );
        Ok(())
    })
}

#[test]
fn test_concat_option_one_of() {
    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard().with(register_select).build();

        let value = to_value(
            &env,
            &globals,
            indoc!(
                r#"
            ["foo"] + select({"DEFAULT": ["bar"]})
            "#
            ),
        );

        let attr = AttrType::option(AttrType::one_of(vec![
            AttrType::list(AttrType::string()),
            AttrType::list(AttrType::bool()),
        ]));

        let coerced = coercion_ctx().coerce_sealed(&attr, value).unwrap();
        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()
            .unwrap();
        assert_eq!(
            r#"["foo", "bar"]"#,
            configured.as_display_no_ctx().to_string()
        );
        buck2_error::Ok(())
    })
    .unwrap();
}

#[test]
fn test_any() -> buck2_error::Result<()> {
    Heap::temp(|heap| {
        let value = heap.alloc(vec!["//some:target", "cell1//named:target[foo]"]);
        let attr = AttrType::any();

        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        assert_eq!(
            "[\"//some:target\", \"cell1//named:target[foo]\"]",
            coerced.as_ref().as_display_no_ctx().to_string()
        );
        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!(
            "[\"//some:target\", \"cell1//named:target[foo]\"]",
            configured.as_display_no_ctx().to_string()
        );

        let value = Value::new_none();
        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        assert_eq!("None", coerced.as_ref().as_display_no_ctx().to_string());
        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!("None", configured.as_display_no_ctx().to_string());

        let value = Value::new_bool(true);
        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        assert_eq!("True", coerced.as_ref().as_display_no_ctx().to_string());
        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!("True", configured.as_display_no_ctx().to_string());

        let value = heap.alloc(42);
        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        assert_eq!("42", coerced.as_ref().as_display_no_ctx().to_string());
        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!("42", configured.as_display_no_ctx().to_string());

        Ok(())
    })
}

#[test]
fn test_option() -> buck2_error::Result<()> {
    Heap::temp(|heap| {
        let attr = AttrType::option(AttrType::list(AttrType::string()));

        let value = heap.alloc(vec!["string1", "string2"]);
        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        assert_eq!(
            "[\"string1\", \"string2\"]",
            coerced.as_ref().as_display_no_ctx().to_string()
        );
        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!(
            "[\"string1\", \"string2\"]",
            configured.as_display_no_ctx().to_string()
        );

        let value = Value::new_none();
        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        assert_eq!("None", coerced.as_ref().as_display_no_ctx().to_string());
        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!("None", configured.as_display_no_ctx().to_string());

        Ok(())
    })
}

#[test]
fn test_dict() -> buck2_error::Result<()> {
    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard().with(register_select).build();
        let value = to_value(&env, &globals, r#"{"b":["1"],"a":[]}"#);

        let attr = AttrType::dict(AttrType::string(), AttrType::list(AttrType::string()), true);
        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        assert_eq!(
            "{\"a\": [], \"b\": [\"1\"]}",
            coerced.as_ref().as_display_no_ctx().to_string()
        );
        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!(
            "{\"a\": [], \"b\": [\"1\"]}",
            configured.as_display_no_ctx().to_string()
        );

        let attr = AttrType::dict(
            AttrType::string(),
            AttrType::list(AttrType::string()),
            false,
        );
        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        assert_eq!(
            "{\"b\": [\"1\"], \"a\": []}",
            coerced.as_ref().as_display_no_ctx().to_string()
        );
        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!(
            "{\"b\": [\"1\"], \"a\": []}",
            configured.as_display_no_ctx().to_string()
        );

        let value = to_value(
            &env,
            &globals,
            r#"{"b":["1"], "a":[]} + select({"DEFAULT": { "c": []}})"#,
        );
        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!(
            r#"{"b": ["1"], "a": [], "c": []}"#,
            configured.as_display_no_ctx().to_string()
        );

        Ok(())
    })
}

#[test]
fn test_one_of() -> buck2_error::Result<()> {
    Heap::temp(|heap| {
        let value = heap.alloc("one");
        let values = heap.alloc(vec!["test", "extra"]);

        let attr = AttrType::one_of(vec![AttrType::string(), AttrType::list(AttrType::string())]);
        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        assert_eq!("\"one\"", coerced.as_ref().as_display_no_ctx().to_string());
        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!("\"one\"", configured.as_display_no_ctx().to_string());

        let coerced = coercion_ctx().coerce_sealed(&attr, values)?;
        assert_eq!(
            "[\"test\", \"extra\"]",
            coerced.as_ref().as_display_no_ctx().to_string()
        );
        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!(
            "[\"test\", \"extra\"]",
            configured.as_display_no_ctx().to_string()
        );

        let attr = AttrType::one_of(Vec::new());
        let coerced = coercion_ctx().coerce_sealed(&attr, value);
        assert!(coerced.is_err());

        Ok(())
    })
}

#[test]
fn test_label() -> buck2_error::Result<()> {
    Heap::temp(|heap| {
        let value = heap.alloc(vec!["//some:target", "cell1//named:target[foo]"]);

        let attr = AttrType::list(AttrType::dep(ProviderIdSet::EMPTY, PluginKindSet::EMPTY));

        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        assert_eq!(
            "[\"root//some:target\", \"cell1//named:target[foo]\"]",
            coerced.as_ref().as_display_no_ctx().to_string()
        );

        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!(
            format!(
                "[\"root//some:target ({})\", \"cell1//named:target[foo] ({})\"]",
                ConfigurationData::testing_new(),
                ConfigurationData::testing_new()
            ),
            configured.as_display_no_ctx().to_string()
        );

        Ok(())
    })
}

#[test]
fn test_coerced_deps() -> buck2_error::Result<()> {
    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard().with(register_select).build();

        let content = indoc!(
            r#"
            ["//some:target", "cell1//named:target[foo]"] + select({
                "//some:config": ["cell1//named:target[bar]"],
                "DEFAULT": ["cell1//:okay"] + select({
                    "cell1//other:config": ["//some:target2"],
                    "DEFAULT": ["//:default1", "//:default2"],
                }),
            }) + ["//:other"]
            "#
        );
        // Check that `x` is captured with the function
        let value = to_value(&env, &globals, content);

        let attr = AttrType::list(AttrType::dep(ProviderIdSet::EMPTY, PluginKindSet::EMPTY));
        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;

        let mut visitor = CoercedDepsCollector::new();
        coerced
            .as_ref()
            .traverse(&attr, Some(PackageLabel::testing()), &mut visitor)?;
        let CoercedDepsCollector {
            deps,
            configuration_deps,
            ..
        } = visitor;
        let deps: Vec<_> = deps.iter().map(|t| t.to_string()).collect();
        let config_deps: Vec<_> = configuration_deps.iter().map(|t| t.0.to_string()).collect();

        let expected_deps = vec![
            "root//some:target",
            "cell1//named:target",
            "cell1//:okay",
            "root//some:target2",
            "root//:default1",
            "root//:default2",
            "root//:other",
        ];

        assert_eq!(expected_deps, deps);

        let expected_config_deps = vec!["root//some:config", "cell1//other:config"];
        assert_eq!(expected_config_deps, config_deps);

        Ok(())
    })
}

#[test]
fn test_configured_deps() -> buck2_error::Result<()> {
    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard().with(register_select).build();

        let content = indoc!(
            r#"
            ["//some:target", "cell1//named:target[foo]"] + select({
                "//some:config": ["cell1//named:target[bar]"],
                "DEFAULT": ["cell1//:okay"] + select({
                    "cell1//other:config": ["//some:target2"],
                    "DEFAULT": ["//:default1", "//:default2"],
                }),
            }) + ["//:other"]
            "#
        );
        // Check that `x` is captured with the function
        let value = to_value(&env, &globals, content);

        let attr = AttrType::list(AttrType::dep(ProviderIdSet::EMPTY, PluginKindSet::EMPTY));
        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;

        let mut info = ConfiguredAttrInfoForTests::new();
        configured.traverse(PackageLabel::testing(), &mut info)?;

        let expected_deps = [
            "root//some:target",
            "cell1//named:target[foo]",
            "cell1//:okay",
            "root//:default1",
            "root//:default2",
            "root//:other",
        ];

        assert_eq!(
            expected_deps
                .iter()
                .map(|s| format!("{} ({})", s, ConfigurationData::testing_new()))
                .collect::<Vec<_>>(),
            info.deps
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );

        // Check also that execution deps are handled slightly differently.
        let attr_exec = AttrType::list(AttrType::exec_dep(ProviderIdSet::EMPTY));
        let coerced_exec = coercion_ctx().coerce_sealed(&attr_exec, value)?;
        let configured_exec = coerced_exec
            .as_ref()
            .configure(&attr_exec, &configuration_ctx(), None)
            .require_compatible()?;
        let mut info = ConfiguredAttrInfoForTests::new();
        configured_exec.traverse(PackageLabel::testing(), &mut info)?;
        eprintln!("{info:?}");
        let exec_cfg = configuration_ctx().base_exec_cfg()?;
        assert_eq!(
            expected_deps
                .iter()
                .map(|s| format!("{s} ({exec_cfg})"))
                .collect::<Vec<_>>(),
            info.execution_deps
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );

        Ok(())
    })
}

#[test]
fn test_resolved_deps() -> buck2_error::Result<()> {
    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard()
            .with(register_select)
            .with(buck2_build_api::interpreter::rule_defs::register_rule_defs)
            .build();

        let content = indoc!(
            r#"
            ["//sub/dir:foo", "//sub/dir:foo[bar]"]
            "#
        );
        // Check that `x` is captured with the function
        let value = to_value(&env, &globals, content);

        let attr = AttrType::list(AttrType::dep(ProviderIdSet::EMPTY, PluginKindSet::EMPTY));
        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        let mut resolution_ctx = resolution_ctx(&env);
        let resolved = configured.resolve_single(PackageLabel::testing(), &mut resolution_ctx)?;

        env.set("res", resolved);
        let content = indoc!(
            r#"
            foo = res[0]
            bar = res[1]
            def assert_eq(a, b):
                if a != b:
                    fail("Expected {} == {}".format(a, b))

            assert_eq(foo[DefaultInfo].sub_targets["bar"][DefaultInfo], bar[DefaultInfo])
            assert_eq(
                ("sub/dir", "foo", None),
                (foo.label.package, foo.label.name, foo.label.sub_target)
            )
            assert_eq(
                ("sub/dir", "foo", ["bar"]),
                (bar.label.package, bar.label.name, bar.label.sub_target),
            )
            None
            "#
        );

        let success = to_value(&env, &globals, content);
        assert!(success.is_none());
        Ok(())
    })
}

#[test]
fn test_dep_requires_providers() -> buck2_error::Result<()> {
    Module::with_temp_heap(|env| {
        let (mut resolution_ctx, provider_ids) = resolution_ctx_with_providers(&env);

        Heap::temp(|heap| {
            let foo_only = heap.alloc("//sub/dir:foo[foo_only]");

            let attr = AttrType::dep(provider_ids.dupe(), PluginKindSet::EMPTY);
            let coerced = coercion_ctx().coerce_sealed(&attr, foo_only)?;
            let configured = coerced
                .as_ref()
                .configure(&attr, &configuration_ctx(), None)
                .require_compatible()?;

            let err = configured
                .resolve_single(PackageLabel::testing(), &mut resolution_ctx)
                .expect_err("Should have failed");
            assert!(
                err.to_string()
                    .contains("Attribute requires a dep that provides `BarInfo`")
            );

            let foo_and_bar = heap.alloc("//sub/dir:foo[foo_and_bar]");

            let attr = AttrType::dep(provider_ids, PluginKindSet::EMPTY);
            let coerced = coercion_ctx().coerce_sealed(&attr, foo_and_bar)?;
            let configured = coerced
                .as_ref()
                .configure(&attr, &configuration_ctx(), None)
                .require_compatible()?;

            // This dep has both FooInfo and BarInfo, so it should resolve properly
            configured.resolve_single(PackageLabel::testing(), &mut resolution_ctx)?;

            Ok(())
        })
    })
}

#[test]
fn test_source_missing() {
    Heap::temp(|heap| {
        let value = heap.alloc(vec!["foo/bar.cpp"]);
        let attr = AttrType::list(AttrType::source(false));

        // FIXME: T85510500 Enable this test properly once we can error out on missing files
        match coercion_ctx().coerce_sealed(&attr, value) {
            Ok(_) => eprintln!("Todo, turn this into an error once T85510500 is fixed"),
            Err(e) => {
                let s = format!("{e:#}");
                assert!(
                    s.contains("Source file `foo/bar.cpp` does not exist"),
                    "Got error {s}"
                )
            }
        }
    })
}

#[test]
fn test_source_label() -> buck2_error::Result<()> {
    Heap::temp(|heap| {
        let value = heap.alloc(vec![
            "//some:target",
            "cell1//named:target[foo]",
            "foo/bar.cpp",
        ]);

        let attr = AttrType::list(AttrType::source(false));

        let coerced = coercion_ctx_listing(PackageListing::testing_files(&["foo/bar.cpp"]))
            .coerce_sealed(&attr, value)?;
        assert_eq!(
            "[\"root//some:target\", \"cell1//named:target[foo]\", \"root//package/subdir/foo/bar.cpp\"]",
            coerced
                .as_ref()
                .as_display(&AttrFmtContext {
                    package: Some(PackageLabel::testing()),
                    options: Default::default(),
                })
                .to_string(),
        );

        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!(
            format!(
                "[{}, {}, {}]",
                format_args!(
                    "\"root//some:target ({})\"",
                    ConfigurationData::testing_new(),
                ),
                format_args!(
                    "\"cell1//named:target[foo] ({})\"",
                    ConfigurationData::testing_new()
                ),
                "\"root//package/subdir/foo/bar.cpp\"",
            ),
            configured
                .as_display(&AttrFmtContext {
                    package: Some(PackageLabel::testing()),
                    options: Default::default(),
                })
                .to_string(),
        );

        Ok(())
    })
}

#[test]
fn test_source_label_deps() -> buck2_error::Result<()> {
    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard().with(register_select).build();

        let content = indoc!(
            r#"
            ["//some:target", "cell1//named:target[foo]", "some/target.cpp"] + select({
                "//some:config": ["cell1//named:target[bar]", "cell1/named/target/bar.cpp"],
                "DEFAULT": ["cell1//:okay", "cell1/okay.cpp"] + select({
                    "cell1//other:config": ["//some:target2", "some/target2.cpp"],
                    "DEFAULT": ["//:default1", "//:default2", "default.cpp"],
                }),
            }) + ["//:other", "other.cpp"]
            "#
        );
        // Check that `x` is captured with the function
        let value = to_value(&env, &globals, content);

        let attr = AttrType::list(AttrType::source(false));
        let coerced = coercion_ctx_listing(PackageListing::testing_files(&[
            "some/target.cpp",
            "cell1/named/target/bar.cpp",
            "cell1/okay.cpp",
            "some/target2.cpp",
            "other.cpp",
            "default.cpp",
        ]))
        .coerce_sealed(&attr, value)?;

        let mut visitor = CoercedDepsCollector::new();
        coerced
            .as_ref()
            .traverse(&attr, Some(PackageLabel::testing()), &mut visitor)?;
        let CoercedDepsCollector {
            deps,
            configuration_deps,
            ..
        } = visitor;
        let deps: Vec<_> = deps.iter().map(|t| t.to_string()).collect();
        let config_deps: Vec<_> = configuration_deps.iter().map(|t| t.0.to_string()).collect();

        let expected_deps = vec![
            "root//some:target",
            "cell1//named:target",
            "cell1//:okay",
            "root//some:target2",
            "root//:default1",
            "root//:default2",
            "root//:other",
        ];

        assert_eq!(expected_deps, deps);

        let expected_config_deps = vec!["root//some:config", "cell1//other:config"];
        assert_eq!(expected_config_deps, config_deps);

        Ok(())
    })
}

#[test]
fn test_source_label_resolution() -> buck2_error::Result<()> {
    fn resolve_and_test(
        content: &str,
        test_content: &str,
        files: &[&str],
    ) -> buck2_error::Result<()> {
        Module::with_temp_heap(|env| {
            let globals = GlobalsBuilder::standard()
                .with(register_select)
                .with(register_builtin_providers)
                .build();

            let value = to_value(&env, &globals, content);

            let attr = AttrType::list(AttrType::source(false));
            let coerced = coercion_ctx_listing(PackageListing::testing_files(files))
                .coerce_sealed(&attr, value)?;
            let configured = coerced
                .as_ref()
                .configure(&attr, &configuration_ctx(), None)
                .require_compatible()?;
            let mut resolution_ctx = resolution_ctx(&env);
            let resolved =
                configured.resolve_single(PackageLabel::testing(), &mut resolution_ctx)?;

            env.set("res", resolved);
            let success = to_value(&env, &globals, test_content);
            assert!(success.is_none());
            Ok(())
        })
    }

    let content = indoc!(r#"["//sub/dir:foo", "//sub/dir:foo[multiple]", "baz/quz.cpp"]"#);
    let test_content = indoc!(
        r#"
            def assert_eq(a, b):
                if a != b:
                    fail("Expected {} == {}".format(a, b))

            expected = ["default.cpp", "bar1.cpp", "bar2.cpp", "bar3.cpp", "quz.cpp"]
            names = [f.basename for f in res]
            assert_eq(expected, names)
            None
            "#
    );
    resolve_and_test(content, test_content, &["baz/quz.cpp"])?;

    let content = indoc!(r#"["//sub/dir:foo", "//sub/dir:foo[single]", "baz/quz.cpp"]"#);
    let test_content = indoc!(
        r#"
            def assert_eq(a, b):
                if a != b:
                    fail("Expected {} == {}".format(a, b))

            expected = ["default.cpp", "bar1.cpp", "quz.cpp"]
            names = [f.basename for f in res]
            assert_eq(expected, names)
            None
            "#
    );
    resolve_and_test(content, test_content, &["baz/quz.cpp"])?;

    let content = indoc!(r#"["//sub/dir:foo", "//sub/dir:foo[zero]", "baz/quz.cpp"]"#);
    let test_content = indoc!(
        r#"
            def assert_eq(a, b):
                if a != b:
                    fail("Expected {} == {}".format(a, b))

            expected = ["default.cpp", "quz.cpp"]
            names = [f.basename for f in res]
            assert_eq(expected, names)
            None
            "#
    );
    resolve_and_test(content, test_content, &["baz/quz.cpp"])
}

#[test]
fn test_single_source_label_fails_if_multiple_returned() -> buck2_error::Result<()> {
    Module::with_temp_heap(|env| {
        Heap::temp(|heap| {
            let value = heap.alloc("//sub/dir:foo[multiple]");

            let attr = AttrType::source(false);
            let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
            let configured = coerced
                .as_ref()
                .configure(&attr, &configuration_ctx(), None)
                .require_compatible()?;
            let mut resolution_ctx = resolution_ctx(&env);
            let err = configured
                .resolve_single(PackageLabel::testing(), &mut resolution_ctx)
                .expect_err("Getting multiple values when expecting a single one should fail");

            assert!(err.to_string().contains("Expected a single artifact"));
            assert!(err.to_string().contains("3 artifacts"));
            Ok(())
        })
    })
}

#[test]
fn test_arg() -> buck2_error::Result<()> {
    Heap::temp(|heap| {
        let value = heap.alloc("$(exe //some:exe) --file=$(location \"//some:location\")");

        let attr = AttrType::arg(false);

        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        // Note that targets are canonicalized.
        assert_eq!(
            "\"$(exe root//some:exe) --file=$(location root//some:location)\"",
            coerced.as_ref().as_display_no_ctx().to_string()
        );
        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!(
            format!(
                "\"$(exe root//some:exe ({})) --file=$(location root//some:location ({}))\"",
                configuration_ctx().base_exec_cfg()?,
                ConfigurationData::testing_new(),
            ),
            configured.as_display_no_ctx().to_string()
        );

        let mut visitor = CoercedDepsCollector::new();
        coerced
            .as_ref()
            .traverse(&attr, Some(PackageLabel::testing()), &mut visitor)?;
        let CoercedDepsCollector {
            deps, exec_deps, ..
        } = visitor;
        let deps: Vec<_> = deps.iter().map(|t| t.to_string()).collect();
        let exec_deps: Vec<_> = exec_deps.iter().map(|t| t.to_string()).collect();

        let mut info = ConfiguredAttrInfoForTests::new();
        configured.traverse(PackageLabel::testing(), &mut info)?;

        let expected_deps = vec!["root//some:location"];
        let expected_exec_deps = vec!["root//some:exe"];
        let expected_configured_deps = vec![format!(
            "root//some:location ({})",
            ConfigurationData::testing_new()
        )];
        let expected_configured_exec_deps = vec![format!(
            "root//some:exe ({})",
            configuration_ctx().base_exec_cfg()?
        )];

        assert_eq!(expected_deps, deps);
        assert_eq!(expected_exec_deps, exec_deps);

        assert_eq!(
            expected_configured_deps,
            info.deps
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            expected_configured_exec_deps,
            info.execution_deps
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );

        Ok(())
    })
}

#[test]
fn test_bool() -> buck2_error::Result<()> {
    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard().with(register_select).build();

        let value = to_value(
            &env,
            &globals,
            indoc!(
                r#"
                (
                    [True, False]
                    + select({
                        "//some:config": [True],
                        "DEFAULT": [False],
                    })
                    + [True]
                )
                "#
            ),
        );

        let attr = AttrType::list(AttrType::bool());

        let coerced = coercion_ctx().coerce_sealed(&attr, value)?;
        assert_eq!(
            "[True, False]+select({\"root//some:config\": [True], \"DEFAULT\": [False]})+[True]",
            coerced.as_ref().as_display_no_ctx().to_string()
        );

        let configured = coerced
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!(
            "[True, False, False, True]",
            configured.as_display_no_ctx().to_string()
        );

        let mut ctx = resolution_ctx(&env);
        let resolved = configured.resolve_single(PackageLabel::testing(), &mut ctx)?;
        assert_eq!("[True, False, False, True]", resolved.to_string());

        Ok(())
    })
}

#[test]
fn test_user_placeholders() -> buck2_error::Result<()> {
    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard()
            .with(register_select)
            .with(register_builtin_providers)
            .build();

        let resolve = |value: &str| {
            let attr = AttrType::arg(false);
            let coerced = coercion_ctx().coerce_sealed(&attr, to_value(&env, &globals, value))?;
            let configured = coerced
                .as_ref()
                .configure(&attr, &configuration_ctx(), None)
                .require_compatible()?;
            let mut resolution_ctx = resolution_ctx(&env);
            configured
                .resolve_single(PackageLabel::testing(), &mut resolution_ctx)
                .map(|v| {
                    // TODO: this is way too unnecessarily verbose for a test.
                    let project_fs = ProjectRoot::new(
                        AbsNormPathBuf::try_from(std::env::current_dir().unwrap()).unwrap(),
                    )
                    .unwrap();
                    let fs = ArtifactFs::new(
                        CellResolver::testing_with_name_and_path(
                            CellName::testing_new("cell"),
                            CellRootPathBuf::new(ProjectRelativePathBuf::unchecked_new(
                                "cell_path".into(),
                            )),
                        ),
                        BuckOutPathResolver::new(ProjectRelativePathBuf::unchecked_new(
                            "buck_out/v2".into(),
                        )),
                        project_fs,
                    );
                    let executor_fs = ExecutorFs::new(&fs, PathSeparatorKind::Unix);

                    let mut cli = Vec::<String>::new();
                    let artifact_path_mapping = BuckMutMap::default();
                    let mut fmt =
                        CommandLineBuilder::new(&mut cli, &artifact_path_mapping, &executor_fs);
                    ValueAsCommandLineLike::unpack_value_err(v)
                        .unwrap()
                        .0
                        .add_to_command_line(&mut fmt)
                        .unwrap();
                    cli.join(" ")
                })
        };

        assert_eq!("clang++", resolve(r#""$(CXX)""#)?);
        assert_eq!(
            "hello",
            resolve(r#""$(user_key //sub/dir:keyed_placeholder)""#)?
        );
        assert_eq!(
            "world",
            resolve(r#""$(key_with_args //sub/dir:keyed_placeholder)""#)?
        );
        assert_eq!(
            "big world",
            resolve(r#""$(key_with_args //sub/dir:keyed_placeholder big)""#)?
        );

        let value = r#""$(CXXabcdef)""#;
        match resolve(value) {
            Ok(..) => panic!("expected error resolving {value}"),
            Err(e) => {
                let expected = "no mapping for CXXabcdef";
                let message = format!("{e:?}");
                assert!(
                    message.contains(expected),
                    "expected `{message}` to contain `{expected}`"
                );
            }
        }

        let value = r#""$(missing_user_key //sub/dir:keyed_placeholder)""#;
        match resolve(value) {
            Ok(..) => panic!("expected error resolving {value}"),
            Err(e) => {
                let expected = "no mapping for missing_user_key";
                let message = format!("{e:?}");
                assert!(
                    message.contains(expected),
                    "expected `{message}` to contain `{expected}`"
                );
            }
        }

        Ok(())
    })
}

/// Errors configuring an attribute, whether materializing it or collecting its
/// deps, are tagged as configuration errors.
#[test]
fn test_configure_errors_are_tagged() -> buck2_error::Result<()> {
    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard().with(register_select).build();
        let value = to_value(&env, &globals, r#"select({"root//some:config": "a"})"#);
        let attr = Attribute::new_const(None, "", AttrType::string());
        let coerced = coercion_ctx().coerce_sealed(attr.coercer(), value)?;
        let full = CoercedAttrFull {
            name: "a",
            attr: &attr,
            value: coerced.as_ref(),
        };

        let err = full
            .configure(&configuration_ctx())
            .require_compatible()
            .expect_err("no condition matches and there is no default");
        assert!(
            err.tags().contains(&ErrorTag::ConfigureAttr),
            "tags: {:?}",
            err.tags()
        );
        let err = full
            .traverse_configured(
                PackageLabel::testing(),
                &configuration_ctx(),
                &mut ConfiguredAttrInfoForTests::new(),
            )
            .require_compatible()
            .expect_err("no condition matches and there is no default");
        assert!(
            err.tags().contains(&ErrorTag::ConfigureAttr),
            "tags: {:?}",
            err.tags()
        );
        Ok(())
    })
}

#[test]
fn test_select_incompatible_configure() -> buck2_error::Result<()> {
    let attr = AttrType::string();
    let coerced = OwnedCoercedAttr::new_with(|b| {
        b.encode_select_incompatible("not supported on this platform")
    })?;
    let result = coerced
        .as_ref()
        .configure(&attr, &configuration_ctx(), None);
    assert!(
        !result.is_compatible(),
        "select_incompatible should produce Incompatible result"
    );
    let err = result.require_compatible().unwrap_err();
    assert!(
        err.to_string().contains("not supported on this platform"),
        "Error message should contain the select_incompatible message, got: {}",
        err
    );
    Ok(())
}

/// A `sorted = True` dict is sorted whether it is a literal or assembled from a concatenation
/// with a `select`.
#[test]
fn test_sorted_dict_concat_is_sorted() -> buck2_error::Result<()> {
    Module::with_temp_heap(|env| {
        let globals = GlobalsBuilder::standard().with(register_select).build();
        let attr = AttrType::dict(AttrType::string(), AttrType::string(), true);

        let literal = to_value(&env, &globals, r#"{"b": "1", "a": "2"}"#);
        let configured = coercion_ctx()
            .coerce_sealed(&attr, literal)?
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!(
            r#"{"a": "2", "b": "1"}"#,
            configured.as_display_no_ctx().to_string()
        );

        let split = to_value(
            &env,
            &globals,
            r#"{"b": "1"} + select({"DEFAULT": {"a": "2"}})"#,
        );
        let configured = coercion_ctx()
            .coerce_sealed(&attr, split)?
            .as_ref()
            .configure(&attr, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!(
            r#"{"a": "2", "b": "1"}"#,
            configured.as_display_no_ctx().to_string()
        );

        // An unsorted dict keeps the operand order.
        let unsorted = AttrType::dict(AttrType::string(), AttrType::string(), false);
        let split = to_value(
            &env,
            &globals,
            r#"{"b": "1"} + select({"DEFAULT": {"a": "2"}})"#,
        );
        let configured = coercion_ctx()
            .coerce_sealed(&unsorted, split)?
            .as_ref()
            .configure(&unsorted, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!(
            r#"{"b": "1", "a": "2"}"#,
            configured.as_display_no_ctx().to_string()
        );

        // `attrs.named_set(attrs.string(), sorted = True)` is
        // `one_of(dict(string, v, sorted = True), list(v))` and behaves the same.
        let named_set = AttrType::one_of(vec![
            AttrType::dict(AttrType::string(), AttrType::string(), true),
            AttrType::list(AttrType::string()),
        ]);
        let split = to_value(
            &env,
            &globals,
            r#"{"b.h": "x/b.h"} + select({"DEFAULT": {"a.h": "y/a.h"}})"#,
        );
        let configured = coercion_ctx()
            .coerce_sealed(&named_set, split)?
            .as_ref()
            .configure(&named_set, &configuration_ctx(), None)
            .require_compatible()?;
        assert_eq!(
            r#"{"a.h": "y/a.h", "b.h": "x/b.h"}"#,
            configured.as_display_no_ctx().to_string()
        );
        Ok(())
    })
}
