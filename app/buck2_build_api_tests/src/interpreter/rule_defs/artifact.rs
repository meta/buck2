/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_interpreter_for_build::interpreter::testing::Tester;
use buck2_interpreter_for_build::interpreter::testing::expect_error;
use indoc::indoc;

use crate::interpreter::rule_defs::artifact::testing::artifactory;

pub(crate) mod testing;

#[test]
fn source_artifact() -> buck2_error::Result<()> {
    let mut tester = Tester::new()?;
    tester.additional_globals(artifactory);
    tester.run_starlark_bzl_test(indoc!(
        r#"
            a1 = source_artifact("foo/bar", "baz/quz.h")
            a2 = source_artifact("foo/bar", "baz/file1")

            def test():
                a3 = source_artifact("foo/bar", "baz/quz.cpp")
                a4 = source_artifact("foo/bar", "baz/file2")

                assert_eq("<source artifact foo/bar/baz/quz.h>", repr(a1))
                assert_eq("quz.h", a1.basename)
                assert_eq("baz/quz.h", a1.short_path)
                assert_eq(".h", a1.extension)
                assert_eq(True, a1.is_source)
                assert_eq(None, a1.owner)

                assert_eq("<source artifact foo/bar/baz/file1>", repr(a2))
                assert_eq("file1", a2.basename)
                assert_eq("baz/file1", a2.short_path)
                assert_eq("", a2.extension)
                assert_eq(True, a2.is_source)
                assert_eq(None, a2.owner)

                assert_eq("<source artifact foo/bar/baz/quz.cpp>", repr(a3))
                assert_eq("quz.cpp", a3.basename)
                assert_eq("baz/quz.cpp", a3.short_path)
                assert_eq(".cpp", a3.extension)
                assert_eq(True, a3.is_source)
                assert_eq(None, a3.owner)

                assert_eq("<source artifact foo/bar/baz/file2>", repr(a4))
                assert_eq("file2", a4.basename)
                assert_eq("baz/file2", a4.short_path)
                assert_eq("", a4.extension)
                assert_eq(True, a4.is_source)
                assert_eq(None, a4.owner)

                # Validate that attrs are setup properly
                for a in (a1, a2, a3, a4):
                    for prop in dir(a):
                        assert_eq(True, hasattr(a, prop))
                        if prop != "as_output":
                            getattr(a, prop)
            "#
    ))?;

    let as_output = indoc!(
        r#"
            def test():
                source_artifact("foo/bar", "baz/quz.cpp").as_output()
            "#
    );
    expect_error(
        tester.run_starlark_bzl_test(as_output),
        as_output,
        "Source artifacts may not be outputs",
    );
    Ok(())
}

#[test]
fn bound_artifact() -> buck2_error::Result<()> {
    let mut tester = Tester::new()?;
    tester.additional_globals(buck2_build_api::interpreter::rule_defs::register_rule_defs);
    tester.additional_globals(artifactory);
    tester.run_starlark_bzl_test(indoc!(
            r#"
            a1 = bound_artifact("//foo:bar", "baz/quz.h")
            a2 = bound_artifact("//foo:bar", "baz/file1")

            def test():
                a3 = bound_artifact("//foo:bar", "baz/quz.cpp")
                a4 = bound_artifact("//foo:bar", "baz/file2")

                assert_eq_ignore_hash("<build artifact baz/quz.h bound to root//foo:bar (<testing>#<HASH>)>", repr(a1))
                assert_eq("quz.h", a1.basename)
                assert_eq("baz/quz.h", a1.short_path)
                assert_eq(".h", a1.extension)
                assert_eq(False, a1.is_source)
                assert_eq("bar", a1.owner.name)

                assert_eq_ignore_hash("<build artifact baz/file1 bound to root//foo:bar (<testing>#<HASH>)>", repr(a2))
                assert_eq("file1", a2.basename)
                assert_eq("baz/file1", a2.short_path)
                assert_eq("", a2.extension)
                assert_eq(False, a2.is_source)
                assert_eq("bar", a2.owner.name)

                assert_eq_ignore_hash("<build artifact baz/quz.cpp bound to root//foo:bar (<testing>#<HASH>)>", repr(a3))
                assert_eq("quz.cpp", a3.basename)
                assert_eq("baz/quz.cpp", a3.short_path)
                assert_eq(".cpp", a3.extension)
                assert_eq(False, a3.is_source)
                assert_eq("bar", a3.owner.name)

                assert_eq_ignore_hash("<build artifact baz/file2 bound to root//foo:bar (<testing>#<HASH>)>", repr(a4))
                assert_eq("file2", a4.basename)
                assert_eq("baz/file2", a4.short_path)
                assert_eq("", a4.extension)
                assert_eq(False, a4.is_source)
                assert_eq("bar", a4.owner.name)

                # Validate that attrs are setup properly
                for a in (a1, a2, a3, a4):
                    for prop in dir(a):
                        assert_eq(True, hasattr(a, prop))
                        if prop != "as_output":
                            getattr(a, prop)
            "#
        ))?;

    let as_output = indoc!(
        r#"
            def test():
                bound_artifact("//foo:bar", "baz/quz.cpp").as_output()
            "#
    );
    expect_error(
        tester.run_starlark_bzl_test(as_output),
        as_output,
        "already used",
    );
    Ok(())
}

#[test]
fn declared_artifact() -> buck2_error::Result<()> {
    let mut tester = Tester::new()?;
    tester.additional_globals(artifactory);
    tester.run_starlark_bzl_test(indoc!(
        r#"
            def test():
                a1 = declared_artifact("baz/quz.cpp")
                a2 = declared_artifact("baz/file2")

                assert_eq("<build artifact baz/quz.cpp>", repr(a1))
                assert_eq("quz.cpp", a1.basename)
                assert_eq(".cpp", a1.extension)
                assert_eq(False, a1.is_source)
                assert_eq(None, a1.owner)

                assert_eq("<build artifact baz/file2>", repr(a2))
                assert_eq("file2", a2.basename)
                assert_eq("", a2.extension)
                assert_eq(False, a2.is_source)
                assert_eq(None, a2.owner)

                # Validate that attrs are setup properly
                for a in (a1, a2):
                    for prop in dir(a):
                        assert_eq(True, hasattr(a, prop))
                        if prop != "as_output":
                            getattr(a, prop)
            "#
    ))?;
    Ok(())
}

#[test]
fn output_artifact() -> buck2_error::Result<()> {
    let mut tester = Tester::new()?;
    tester.additional_globals(artifactory);
    tester.run_starlark_bzl_test(indoc!(
        r#"
            b = declared_bound_artifact("//foo:bar", "baz/quz.h")
            frozen_b = b.as_output()

            def test():
                a1 = declared_artifact("baz/quz.cpp")
                a1o = a1.as_output()
                a2 = declared_artifact("baz/file2")
                a2o = a2.as_output()

                assert_eq(a1o.as_input(), a1)

                assert_eq("<output artifact for baz/quz.cpp>", repr(a1o))
                assert_eq("quz.cpp", a1o.basename)
                assert_eq(".cpp", a1o.extension)
                assert_eq(False, a1o.is_source)
                assert_eq(None, a1o.owner)

                assert_eq("<output artifact for baz/file2>", repr(a2o))
                assert_eq("file2", a2o.basename)
                assert_eq("", a2o.extension)
                assert_eq(False, a2o.is_source)
                assert_eq(None, a2o.owner)

                # Sanity check that methods are also available on frozen artifacts
                assert_eq(False, frozen_b.is_source)

                # Validate that attrs are setup properly
                for a in (a1o, a2o):
                    for prop in dir(a):
                        assert_eq(True, hasattr(a, prop))
                        getattr(a, prop)

                # Check that output artifacts compare equal to each other but not their non-output
                # versions
                assert_eq(a1.as_output(), a1.as_output())
                assert_ne(a1.as_output(), a1)

                # Check hashable
                d = {a1o: 1}
            "#
    ))?;
    Ok(())
}

#[test]
fn declared_bound() -> buck2_error::Result<()> {
    let mut tester = Tester::new()?;
    tester.additional_globals(buck2_build_api::interpreter::rule_defs::register_rule_defs);
    tester.additional_globals(artifactory);
    tester.run_starlark_bzl_test(indoc!(
            r#"
            a1 = declared_bound_artifact("//foo:bar", "baz/quz.h")
            a2 = declared_bound_artifact("//foo:bar", "baz/file1")

            def test():
                a3 = declared_bound_artifact("//foo:bar", "baz/quz.cpp")
                a4 = declared_bound_artifact("//foo:bar", "baz/file2")

                assert_eq_ignore_hash("<build artifact baz/quz.h bound to root//foo:bar (<testing>#<HASH>)>", repr(a1))
                assert_eq("quz.h", a1.basename)
                assert_eq("baz/quz.h", a1.short_path)
                assert_eq(".h", a1.extension)
                assert_eq(False, a1.is_source)
                assert_eq("bar", a1.owner.name)

                assert_eq_ignore_hash("<build artifact baz/file1 bound to root//foo:bar (<testing>#<HASH>)>", repr(a2))
                assert_eq("file1", a2.basename)
                assert_eq("baz/file1", a2.short_path)
                assert_eq("", a2.extension)
                assert_eq(False, a2.is_source)
                assert_eq("bar", a2.owner.name)

                assert_eq_ignore_hash("<build artifact baz/quz.cpp bound to root//foo:bar (<testing>#<HASH>)>", repr(a3))
                assert_eq("quz.cpp", a3.basename)
                assert_eq("baz/quz.cpp", a3.short_path)
                assert_eq(".cpp", a3.extension)
                assert_eq(False, a3.is_source)
                assert_eq("bar", a3.owner.name)

                assert_eq_ignore_hash("<build artifact baz/file2 bound to root//foo:bar (<testing>#<HASH>)>", repr(a4))
                assert_eq("file2", a4.basename)
                assert_eq("baz/file2", a4.short_path)
                assert_eq("", a4.extension)
                assert_eq(False, a4.is_source)
                assert_eq("bar", a4.owner.name)

                # Validate that attrs are setup properly
                for a in (a1, a2, a3, a4):
                    for prop in dir(a):
                        assert_eq(True, hasattr(a, prop))
                        if prop != "as_output":
                            getattr(a, prop)
            "#
        ))?;

    Ok(())
}

#[test]
fn project_declared_artifact() -> buck2_error::Result<()> {
    let mut tester = Tester::new()?;
    tester.additional_globals(artifactory);
    tester.run_starlark_bzl_test(indoc!(
            r#"
            def test():
                source = source_artifact("foo/bar", "src").project("baz.cpp")
                assert_eq("<source artifact foo/bar/src/baz.cpp>", repr(source))
                assert_eq("baz.cpp", source.basename)
                assert_eq(".cpp", source.extension)

                bound = bound_artifact("//foo:bar", "baz").project("quz.h")
                assert_eq_ignore_hash("<build artifact baz/quz.h bound to root//foo:bar (<testing>#<HASH>)>", repr(bound))
                assert_eq("quz.h", bound.basename)
                assert_eq(".h", bound.extension)

                bound = declared_bound_artifact("//foo:bar", "out").project("baz.o")
                assert_eq_ignore_hash("<build artifact out/baz.o bound to root//foo:bar (<testing>#<HASH>)>", repr(bound))
                assert_eq("baz.o", bound.basename)
                assert_eq(".o", bound.extension)

                unbound = declared_artifact("out").project("qux.so")
                assert_eq("<build artifact out/qux.so>", repr(unbound))
                assert_eq("<output artifact for out/qux.so>", repr(unbound.as_output()))
                assert_eq("qux.so", unbound.basename)
                assert_eq(".so", unbound.extension)
            "#
        ))?;
    Ok(())
}

#[test]
fn test_short_path() -> buck2_error::Result<()> {
    let mut tester = Tester::new()?;
    tester.additional_globals(artifactory);
    tester.run_starlark_bzl_test(indoc!(
            r#"
            def test():
                test = declared_artifact("foo/bar/baz")
                assert_eq("foo/bar/baz", test.short_path)

                test = declared_artifact("foo").project("bar/baz")
                assert_eq("foo/bar/baz", test.short_path)

                test = declared_artifact("foo").project("bar").project("baz")
                assert_eq("foo/bar/baz", test.short_path)

                test = declared_artifact("foo").project("bar/baz", hide_prefix=True)
                assert_eq("bar/baz", test.short_path)

                test = declared_artifact("foo").project("bar").project("baz", hide_prefix=True)
                assert_eq("baz", test.short_path)

                test = declared_artifact("foo").project("bar", hide_prefix=True).project("baz")
                assert_eq("bar/baz", test.short_path)

                test = declared_artifact("foo").project("bar", hide_prefix=True).project("baz", hide_prefix=True)
                assert_eq("baz", test.short_path)
            "#
        ))?;
    Ok(())
}

#[test]
fn stringifies_for_command_line() -> buck2_error::Result<()> {
    let mut tester = Tester::new()?;
    tester.additional_globals(artifactory);
    tester.run_starlark_bzl_test(indoc!(
            r#"
            a1 = bound_artifact("//foo:bar", "baz/quz.h")
            a2 = source_artifact("foo/bar", "baz/file1")

            def test():
                a3 = bound_artifact("//foo:bar", "baz/quz.cpp")
                a4 = source_artifact("foo/bar", "baz/file2")

                assert_eq_ignore_hash("buck-out/v2/art/root/<HASH>/foo/__bar__/baz/quz.h", stringify_for_cli(a1))
                assert_eq("foo/bar/baz/file1", stringify_for_cli(a2))
                assert_eq_ignore_hash("buck-out/v2/art/root/<HASH>/foo/__bar__/baz/quz.cpp", stringify_for_cli(a3))
                assert_eq("foo/bar/baz/file2", stringify_for_cli(a4))
            "#
        ))
}

#[test]
fn bound_artifact_with_associated_artifacts() -> buck2_error::Result<()> {
    let mut tester = Tester::new()?;
    tester.additional_globals(buck2_build_api::interpreter::rule_defs::register_rule_defs);
    tester.additional_globals(artifactory);
    tester.run_starlark_bzl_test(indoc!(
        r#"
            def test():
                # declare an artifact (a2) with string and add an associated artifact (a1)
                a1 = source_artifact("foo/bar", "baz/file1")
                a2 = declared_bound_artifact_with_associated_artifacts("baz/quz.h", [a1])
                assert_eq(a2.short_path, "baz/quz.h")
                assert_eq(get_associated_artifacts_as_string(a1), "")
                assert_eq(get_associated_artifacts_as_string(a2), "root//foo/bar/baz/file1")

                # use a predeclared artifact (a3) and add an associated artifact (a4)
                a3 = declared_artifact("wom/bat.h")
                a4 = source_artifact("foo/bar", "baz/file2")
                a5 = declared_bound_artifact_with_associated_artifacts(a3, [a4])
                assert_eq(a3.short_path, "wom/bat.h")
                assert_eq(a5.short_path, "wom/bat.h")
                assert_eq(get_associated_artifacts_as_string(a3), "")
                assert_eq(get_associated_artifacts_as_string(a5), "root//foo/bar/baz/file2")

                # use a predeclared artifact (a3) with no associated artifacts
                a6 = declared_bound_artifact_with_associated_artifacts(a3, [])
                assert_eq(a6.short_path, "wom/bat.h")
                assert_eq(get_associated_artifacts_as_string(a6), "")

                a7 = a5.without_associated_artifacts()
                assert_eq(a5.short_path, a7.short_path)
                assert_eq(get_associated_artifacts_as_string(a7), "")
            "#
    ))
}
