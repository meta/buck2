/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_api::interpreter::rule_defs::provider::registration::register_builtin_providers;
use buck2_build_api::interpreter::rule_defs::register_rule_defs;
use buck2_common::legacy_configs::cells::BuckConfigBasedCells;
use buck2_common::legacy_configs::configs::testing::TestConfigParserFileOps;
use buck2_common::package_listing::listing::PackageListing;
use buck2_common::package_listing::listing::testing::PackageListingExt;
use buck2_core::build_file_path::BuildFilePath;
use buck2_core::bzl::ImportPath;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::cell_path_with_allowed_relative_dir::CellPathWithAllowedRelativeDir;
use buck2_interpreter::file_loader::LoadedModules;
use buck2_interpreter::paths::module::OwnedStarlarkModulePath;
use buck2_interpreter::paths::path::StarlarkPath;
use buck2_interpreter_for_build::interpreter::testing::CellsData;
use buck2_interpreter_for_build::interpreter::testing::Tester;
use buck2_interpreter_for_build::interpreter::testing::run_simple_starlark_test;
use buck2_node::attrs::inspect_options::AttrInspectOptions;
use buck2_node::nodes::unconfigured::testing::targets_to_json;
use dupe::Dupe;
use indoc::indoc;
use serde_json::json;

#[test]
fn test_eval_import() {
    let loaded = Tester::new()
        .unwrap()
        .add_import(
            &ImportPath::testing_new("root//some/package:defs.bzl"),
            indoc!(
                r#"
            one = 1
            hello = "world"
            "#
            ),
        )
        .unwrap();

    assert_eq!(
        1,
        loaded
            .env()
            .get("one")
            .unwrap()
            .as_ref()
            .value()
            .unpack_i32()
            .unwrap()
    );

    assert_eq!(
        "world",
        loaded
            .env()
            .get("hello")
            .unwrap()
            .as_ref()
            .value()
            .unpack_str()
            .unwrap()
    );
}

#[test]
fn test_load() {
    let import_path = ImportPath::testing_new("root//imports:one.bzl");
    let mut tester = Tester::new().unwrap();
    tester
        .add_import(
            &import_path,
            indoc!(
                r#"
                    def concat(*args):
                      s = ""
                      for a in args:
                        s += a
                      return s
                    "#
            ),
        )
        .unwrap();

    let parse_result = tester
        .add_import(
            &ImportPath::testing_new("root//some/package:defs.bzl"),
            indoc!(
                r#"
                load("@root//imports:one.bzl", "concat")
                message = concat("hello", " ", "world!")
                "#
            ),
        )
        .unwrap();

    assert_eq!(
        "hello world!",
        parse_result
            .env()
            .get("message")
            .unwrap()
            .as_ref()
            .value()
            .unpack_str()
            .unwrap()
    );
}

#[test]
fn test_eval_build_file() {
    let mut tester = Tester::new().unwrap();
    tester.additional_globals(register_builtin_providers);

    tester
        .add_import(
            &ImportPath::testing_new("root//:rules.bzl"),
            indoc!(
                r#"
                def _impl(ctx):
                    return DefaultInfo()

                export_file = rule(
                    impl = _impl,
                    attrs = {
                        "src": attrs.any(),
                    },
                )

                java_library = rule(
                    impl = _impl,
                    attrs = {
                        "srcs": attrs.list(attrs.any()),
                    },
                )
            "#
            ),
        )
        .unwrap();

    tester
        .add_import(
            &ImportPath::testing_new("root//imports:one.bzl"),
            indoc!(
                r#"
                    load("@root//:rules.bzl", "export_file")

                    def some_macro(name, **kwargs):
                        export_file(
                            name=name+"-exported",
                            **kwargs
                        )
                    "#
            ),
        )
        .unwrap();

    let build_path = BuildFilePath::testing_new("root//some/package:BUILD");
    let eval_result = tester
        .eval_build_file(
            &build_path,
            indoc!(
                r#"
                load("@root//imports:one.bzl", "some_macro")
                load("@root//:rules.bzl", "java_library")

                some_macro(
                    name = "invoke_some",
                    src = "some.file",
                )
                java_library(
                    name = "java",
                    srcs = glob(["**/*.java"]),
                )
                "#
            ),
            PackageListing::testing_files(&["file1.java", "file2.java"]),
        )
        .unwrap();

    assert_eq!(build_path.package(), eval_result.package());
    let target_names = eval_result
        .targets()
        .keys()
        .map(|t| t.as_str().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(vec!["invoke_some-exported", "java"], target_names);
}

fn cells(cell_segmentation: bool) -> CellsData {
    let disable_cell_segmentation = !cell_segmentation;
    let BuckConfigBasedCells {
        cell_resolver,
        root_config,
        ..
    } = futures::executor::block_on(BuckConfigBasedCells::testing_parse_with_file_ops(
        &mut TestConfigParserFileOps::new(&[(
            ".buckconfig",
            &indoc::formatdoc!(
                r#"
                    [cells]
                        root = .
                        cell1 = project/cell1
                        cell2 = project/cell2
                        xalias2 = project/cell2
                    [buck2]
                        disable_cell_segmentation = {disable_cell_segmentation:?}
                    "#
            ),
        )])
        .unwrap(),
        &[],
    ))
    .unwrap();
    (
        cell_resolver.root_cell_cell_alias_resolver().dupe(),
        cell_resolver,
        root_config,
        CellPathWithAllowedRelativeDir::new(
            CellPath::testing_new("cell1//config/foo"),
            Some(CellPath::testing_new("cell1//config")),
        ),
    )
}

#[test]
fn test_find_imports_segmented() {
    let cell_segmentation = true;
    let tester = Tester::with_cells(cells(cell_segmentation)).unwrap();
    let path = BuildFilePath::testing_new("cell1//config/foo:BUCK");
    let parse_result = tester.parse(
        StarlarkPath::BuildFile(&path),
        indoc!(
            r#"
            a = 1
        "#
        ),
    );

    assert!(parse_result.imports().is_empty());

    let parse_result = tester.parse(
        StarlarkPath::BuildFile(&path),
        indoc!(
            r#"
            # some documentation
            """ and a string """

            load("//imports:one.bzl", "some_macro")
            load("@cell1//:one.bzl", "some_macro")
            load("@xalias2//:two.bzl", "some_macro")

            # some other comments
            load(":other.bzl", "some_macro")
            load("../bar/three.bzl", "some_macro")
        "#
        ),
    );

    // cell_segmentation is on, so we have the @cell1 when importing cross-cell
    assert_eq!(
        &[
            "root//imports/one.bzl@cell1",
            "cell1//one.bzl",
            "cell2//two.bzl@cell1",
            "cell1//config/foo/other.bzl",
            "cell1//config/bar/three.bzl",
        ],
        parse_result
            .imports()
            .iter()
            .map(|e| e.1.to_string())
            .collect::<Vec<_>>()
            .as_slice()
    );
}

#[test]
fn test_find_imports_unsegmented() {
    let cell_segmentation = false;
    let tester = Tester::with_cells(cells(cell_segmentation)).unwrap();
    let path = BuildFilePath::testing_new("cell1//config/foo:BUCK");
    let parse_result = tester.parse(
        StarlarkPath::BuildFile(&path),
        indoc!(
            r#"
            a = 1
        "#
        ),
    );

    assert!(parse_result.imports().is_empty());

    let parse_result = tester.parse(
        StarlarkPath::BuildFile(&path),
        indoc!(
            r#"
            # some documentation
            """ and a string """

            load("//imports:one.bzl", "some_macro")
            load("@cell1//:one.bzl", "some_macro")
            load("@xalias2//:two.bzl", "some_macro")

            # some other comments
            load(":other.bzl", "some_macro")
            load("../bar/three.bzl", "some_macro")
        "#
        ),
    );

    // cell_segmentation is off, so we don't get the @cell1 cross-cell
    // indication, just one global canonical import path per cell path.
    assert_eq!(
        &[
            "root//imports/one.bzl",
            "cell1//one.bzl",
            "cell2//two.bzl",
            "cell1//config/foo/other.bzl",
            "cell1//config/bar/three.bzl",
        ],
        parse_result
            .imports()
            .iter()
            .map(|e| e.1.to_string())
            .collect::<Vec<_>>()
            .as_slice()
    );
}

#[test]
fn test_root_import() {
    let mut tester = Tester::with_cells(
        buck2_interpreter_for_build::interpreter::testing::cells(Some(indoc!(
            r#"
            [buildfile]
                includes = //include.bzl
        "#
        )))
        .unwrap(),
    )
    .unwrap();

    tester.additional_globals(register_builtin_providers);

    let import_path = ImportPath::testing_new("root//:include.bzl");
    tester
        .add_import(
            &import_path,
            indoc!(
                r#"
                    some_var = 1
                    def some_func():
                       return "hello"

                    def _impl(ctx):
                        return DefaultInfo()

                    export_file = rule(
                        impl = _impl,
                        attrs = {
                            "level": attrs.int(),
                        },
                    )
        "#
            ),
        )
        .unwrap();

    let build_path = BuildFilePath::testing_new("root//some/package:BUCK");
    let eval_result = tester
        .eval_build_file(
            &build_path,
            indoc!(
                r#"
                export_file(
                    name = some_func(),
                    level = some_var,
                )
                "#
            ),
            PackageListing::testing_files(&["file1.java", "file2.java"]),
        )
        .unwrap();

    assert_eq!(build_path.package(), eval_result.package());
    let target_names = eval_result
        .targets()
        .keys()
        .map(|t| t.as_str().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(vec!["hello"], target_names);
}

#[test]
fn prelude_is_included() -> buck2_error::Result<()> {
    let mut tester = Tester::new()?;
    let prelude_path = ImportPath::testing_new("root//prelude:prelude.bzl");
    tester.set_prelude(prelude_path.clone());

    let prelude = tester.eval_import(&prelude_path, "some_var = 1", LoadedModules::default())?;
    let loaded_modules = LoadedModules {
        map: [(OwnedStarlarkModulePath::LoadFile(prelude_path), prelude)]
            .into_iter()
            .collect(),
    };

    // The prelude should be included in build files, and in .bzl files that are not in the
    // prelude's package
    let build_file = BuildFilePath::testing_new("root//prelude:TARGETS.v2");
    assert!(
        tester
            .eval_build_file_with_loaded_modules(
                &build_file,
                "other_var = some_var",
                loaded_modules.clone(),
                PackageListing::testing_empty()
            )
            .is_ok(),
        "build files in the prelude package should have access to the prelude"
    );

    let import = ImportPath::testing_new("root//not_prelude:sibling.bzl");
    assert!(
        tester
            .eval_import(&import, "other_var = some_var", loaded_modules.clone())
            .is_ok(),
        ".bzl files not in the prelude package should have access to the prelude"
    );

    let import = ImportPath::testing_new("root//prelude:defs.bzl");
    assert!(
        tester
            .eval_import(&import, "other_var = some_var", loaded_modules)
            .is_err(),
        "bzl files in the prelude package should NOT have access to the prelude"
    );

    Ok(())
}

#[test]
fn test_package_import() -> buck2_error::Result<()> {
    let mut tester = Tester::with_cells(buck2_interpreter_for_build::interpreter::testing::cells(
        Some(indoc!(
            r#"
            [buildfile]
                package_includes = src=>//include.bzl::func_alias=some_func
        "#
        )),
    )?)?;

    let import_path = ImportPath::testing_new("root//:include.bzl");
    tester.add_import(
        &import_path,
        indoc!(
            r#"
            def _impl(ctx):
                pass
            export_file = rule(impl=_impl, attrs = {})

            def some_func(name):
                export_file(name = name)
        "#
        ),
    )?;

    let build_path = BuildFilePath::testing_new("root//src/package:BUCK");
    let eval_result = tester.eval_build_file(
        &build_path,
        indoc!(
            r#"
                implicit_package_symbol("func_alias")(
                    implicit_package_symbol("missing", "DEFAULT")
                )
                "#
        ),
        PackageListing::testing_files(&["file1.java", "file2.java"]),
    )?;
    assert_eq!(build_path.package(), eval_result.package());
    assert_eq!(
        json!({
                "DEFAULT": {
                    "__type__": "root//include.bzl:export_file",
                    "compatible_with": [],
                    "default_target_platform": null,
                    "exec_compatible_with": [],
                    "name": "DEFAULT",
                    "target_compatible_with": [],
                    "modifiers": [],
                    "tests": [],
                    "visibility": [],
                    "within_view": ["PUBLIC"],
                    "metadata": {},
                },
        }),
        targets_to_json(
            eval_result.targets(),
            build_path.package(),
            AttrInspectOptions::All
        )?
    );
    Ok(())
}

#[test]
fn eval() -> buck2_error::Result<()> {
    let mut tester = Tester::new()?;
    let content = indoc!(
        r#"
            def _impl(ctx):
                pass
            export_file = rule(impl=_impl, attrs = {})

            def test():
                assert_eq("some/package", __buck2_builtins__.package_name())
                assert_eq("@root", __buck2_builtins__.repository_name())

                assert_eq(package_name(), __buck2_builtins__.package_name())
                assert_eq(repository_name(), __buck2_builtins__.repository_name())

                assert_eq(package_name(), get_base_path())

                export_file(name = "rule_name")
                assert_eq(True, rule_exists("rule_name"))
                assert_eq(False, rule_exists("not_rule_name"))

                print("some message")
                print("multiple", "strings")
            "#
    );
    tester.run_starlark_test(content)?;
    Ok(())
}

#[test]
fn test_builtins() -> buck2_error::Result<()> {
    // Test that most things end up on __buck2_builtins__
    run_simple_starlark_test(indoc!(
        r#"
            def test():
                assert_eq(__buck2_builtins__.json.encode({}), "{}")
            "#
    ))?;

    // But not internals
    let mut tester = Tester::new().unwrap();
    tester.run_starlark_test_expecting_error(
        indoc!(
            r#"
            def test():
                __buck2_builtins__.buck2_fail("message")
            "#
        ),
        "The attribute `buck2_fail` is not available",
    );
    Ok(())
}

#[test]
fn test_oncall() -> buck2_error::Result<()> {
    let mut tester = Tester::new().unwrap();
    tester.additional_globals(register_rule_defs);
    tester.run_starlark_test(indoc!(
        r#"
            def _impl(ctx):
                pass
            export_file = rule(impl=_impl, attrs = {})

            def test():
                oncall("valid")
                export_file(name = "rule_name")
            "#
    ))?;
    tester.run_starlark_test(indoc!(
        r#"
            def _impl(ctx):
                pass
            export_file = rule(impl=_impl, attrs = {})

            def test():
                oncall("valid")
                if read_oncall() != "valid":
                    fail("oncall should be set to valid")
                export_file(name = "rule_name")
                if read_oncall() != "valid":
                    fail("oncall should be set to valid and targets set")
            "#
    ))?;
    tester.run_starlark_test(indoc!(
        r#"
            def _impl(ctx):
                pass
            export_file = rule(impl=_impl, attrs = {})

            def test():
                if read_oncall() != None:
                    fail("oncall should be None if never set")
            "#
    ))?;
    tester.run_starlark_test_expecting_error(
        indoc!(
            r#"
            def test():
                oncall("valid")
                oncall("twice")
            "#
        ),
        "more than once",
    );
    tester.run_starlark_test_expecting_error(
        indoc!(
            r#"
            def _impl(ctx):
                pass
            export_file = rule(impl=_impl, attrs = {})

            def test():
                export_file(name = "rule_name")
                oncall("failure after")
            "#
        ),
        "after one or more targets",
    );
    tester.run_starlark_test_expecting_error(
        indoc!(
            r#"
            def test():
                read_oncall()
                oncall("valid")
            "#
        ),
        "after calling `read_oncall`",
    );
    Ok(())
}

/// `rule_exists()` only asks whether a target has been declared and leaves the file in the
/// "before targets" state, so an `oncall()` after it is accepted.
#[test]
fn test_oncall_after_rule_exists_without_targets() -> buck2_error::Result<()> {
    let mut tester = Tester::new().unwrap();
    tester.additional_globals(register_rule_defs);
    tester.run_starlark_test(indoc!(
        r#"
            def _impl(ctx):
                pass
            export_file = rule(impl=_impl, attrs = {})

            def test():
                if rule_exists("rule_name"):
                    fail("no target declared yet")
                oncall("valid")
                export_file(name = "rule_name")
                if read_oncall() != "valid":
                    fail("oncall should be valid")
                if not rule_exists("rule_name"):
                    fail("target declared")
            "#
    ))?;
    Ok(())
}

/// The same with a string that is not a valid target name.
#[test]
fn test_oncall_after_rule_exists_with_invalid_name() -> buck2_error::Result<()> {
    let mut tester = Tester::new().unwrap();
    tester.additional_globals(register_rule_defs);
    tester.run_starlark_test(indoc!(
        r#"
            def _impl(ctx):
                pass
            export_file = rule(impl=_impl, attrs = {})

            def test():
                if rule_exists("not/a:valid name"):
                    fail("invalid name cannot exist")
                oncall("valid")
                export_file(name = "rule_name")
                if read_oncall() != "valid":
                    fail("oncall should be valid")
            "#
    ))?;
    Ok(())
}
