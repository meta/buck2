/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_api::interpreter::rule_defs::provider::collection::tester::collection_creator;
use buck2_build_api::interpreter::rule_defs::register_rule_defs;
use buck2_interpreter_for_build::interpreter::testing::Tester;
use indoc::indoc;

use crate::interpreter::rule_defs::artifact::testing::artifactory;
use crate::interpreter::rule_defs::cmd_args::testing::command_line_stringifier;

fn run_info_tester() -> Tester {
    let mut tester = Tester::new().unwrap();
    tester.additional_globals(command_line_stringifier);
    tester.additional_globals(artifactory);
    tester.additional_globals(register_rule_defs);
    tester.additional_globals(collection_creator);
    tester
}

#[test]
fn run_info_stringifies() -> buck2_error::Result<()> {
    let mut tester = run_info_tester();
    let content = indoc!(
        r#"
            a = source_artifact("foo/bar", "baz.h")
            b = source_artifact("foo/bar", "baz.cpp")
            c = source_artifact("foo/bar", "baz-inl.h")
            cli = cmd_args().add(a).add(b)
            frozen_ri1 = RunInfo()
            frozen_ri2 = RunInfo(args=["1", "2", cli, a])
            frozen_ri3 = RunInfo(args="1")
            frozen_ri4 = RunInfo(args=cli)
            frozen_ri5 = RunInfo(args=[frozen_ri2, "some_string"])

            cli = cli.copy()
            cli.add(c)

            def test():
                ri1 = RunInfo()
                ri2 = RunInfo(args=["1", "2", cli, a])
                ri3 = RunInfo(args="1")
                ri4 = RunInfo(args=cli)
                ri5 = RunInfo(args=[ri2, "some_string"])
                ri6 = RunInfo(args=[frozen_ri2, "some_string"])

                baz_h = "foo/bar/baz.h"
                baz_cpp = "foo/bar/baz.cpp"
                baz_inl = "foo/bar/baz-inl.h"

                assert_eq([], get_args(frozen_ri1))

                assert_eq(
                    ["1","2", baz_h, baz_cpp, baz_h],
                    get_args(frozen_ri2)
                )

                assert_eq(["1"], get_args(frozen_ri3))

                assert_eq([baz_h, baz_cpp], get_args(frozen_ri4))

                assert_eq(
                    ["1","2", baz_h, baz_cpp, baz_h, "some_string"],
                    get_args(frozen_ri5)
                )

                assert_eq([], get_args(ri1))

                assert_eq(
                    ["1","2", baz_h, baz_cpp, baz_inl, baz_h],
                    get_args(ri2)
                )
                assert_eq(["1"], get_args(ri3))
                assert_eq([baz_h, baz_cpp, baz_inl], get_args(ri4))
                assert_eq(
                    ["1","2", baz_h, baz_cpp, baz_inl, baz_h, "some_string"],
                    get_args(ri5)
                )
                assert_eq(
                    ["1","2", baz_h, baz_cpp, baz_h, "some_string"],
                    get_args(ri6)
                )
            "#
    );
    tester.run_starlark_bzl_test(content)
}

#[test]
fn run_info_validates_types_1() {
    let content_bad_args1 = indoc!(
        r#"
            def test():
                RunInfo(args=False)
            "#
    );
    let mut tester = run_info_tester();
    tester.run_starlark_bzl_test_expecting_error(
        content_bad_args1,
        "Expected type `list | CmdArgsLike` but got",
    );
}

#[test]
fn run_info_validates_types_2() {
    let content_bad_args2 = indoc!(
        r#"
            def test():
                RunInfo(args={})
            "#
    );
    let mut tester = run_info_tester();
    tester.run_starlark_bzl_test_expecting_error(
        content_bad_args2,
        "Expected type `list | CmdArgsLike` but got",
    );
}

#[test]
fn run_info_works_as_provider_key() -> buck2_error::Result<()> {
    let mut tester = run_info_tester();

    let content = indoc!(
        r#"
            c = create_collection([RunInfo(args=["1","2","3"]), DefaultInfo()])
            def test():
                assert_eq(True, contains_provider(c, RunInfo))
            "#
    );

    tester.run_starlark_bzl_test(content)
}
