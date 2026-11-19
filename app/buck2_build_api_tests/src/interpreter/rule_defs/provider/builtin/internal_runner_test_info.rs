/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_api::interpreter::rule_defs::provider::builtin::internal_runner_test_info::InternalRunnerTestInfo;
use buck2_build_api::interpreter::rule_defs::provider::builtin::internal_runner_test_info::OwnedInternalRunnerTestInfo;
use buck2_build_api::interpreter::rule_defs::register_rule_defs;
use buck2_build_api::interpreter::rule_defs::required_test_local_resource::register_required_test_local_resource;
use buck2_core::bzl::ImportPath;
use buck2_interpreter_for_build::interpreter::testing::Tester;
use buck2_test_api::data::TestStatus;
use indoc::indoc;

fn tester() -> Tester {
    let mut tester = Tester::new().unwrap();
    tester.additional_globals(register_rule_defs);
    tester.additional_globals(register_required_test_local_resource);
    tester
}

fn freeze_provider(starlark_code: &str) -> buck2_error::Result<OwnedInternalRunnerTestInfo> {
    let mut tester = tester();
    let loaded = tester.add_import(
        &ImportPath::testing_new("root//test:provider.bzl"),
        starlark_code,
    )?;
    loaded
        .env()
        .get("exported_info")
        .expect("`exported_info` not found")
        .downcast_starlark::<InternalRunnerTestInfo>()
        .map_err(buck2_error::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------------
    // Construction: both callbacks are required alongside `type`
    // ---------------------------------------------------------------------------

    #[test]
    fn test_minimal_construction() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test(indoc!(
            r#"
        def test():
            InternalRunnerTestInfo(
                type = "custom",
                parse_test_listing = lambda stdout: [],
                parse_test_result = lambda stdout, stderr, exit_code: [],
                listing_command = ["binary", "--list"],
            )
        "#
        ))?;
        Ok(())
    }

    #[test]
    fn test_construction_with_optional_fields() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test(indoc!(
            r#"
        def test():
            InternalRunnerTestInfo(
                type = "custom",
                parse_test_listing = lambda stdout: [],
                parse_test_result = lambda stdout, stderr, exit_code: [],
                listing_command = ["my_test_binary", "--list"],
                command = ["my_test_binary"],
                env = {"FOO": "bar"},
                labels = ["slow", "integration"],
                contacts = ["oncall+my_team@xmail.facebook.com"],
                use_project_relative_paths = True,
                run_from_project_root = False,
            )
        "#
        ))?;
        Ok(())
    }

    #[test]
    fn test_construction_with_cmd_args() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test(indoc!(
            r#"
        def test():
            InternalRunnerTestInfo(
                type = "custom",
                parse_test_listing = lambda stdout: [],
                parse_test_result = lambda stdout, stderr, exit_code: [],
                listing_command = ["binary", "--list"],
                command = ["binary", cmd_args()],
                env = {"KEY": cmd_args()},
            )
        "#
        ))?;
        Ok(())
    }

    #[test]
    fn test_construction_with_local_resources() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test(indoc!(
            r#"
        def test():
            InternalRunnerTestInfo(
                type = "custom",
                parse_test_listing = lambda stdout: [],
                parse_test_result = lambda stdout, stderr, exit_code: [],
                listing_command = ["binary", "--list"],
                local_resources = {"gpu": None},
                required_local_resources = [RequiredTestLocalResource("gpu", listing=False)],
            )
        "#
        ))?;
        Ok(())
    }

    // ---------------------------------------------------------------------------
    // Validation: missing required parameters
    // ---------------------------------------------------------------------------

    #[test]
    fn test_missing_type() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                )
            "#
            ),
            "Missing required parameter",
        );
        Ok(())
    }

    #[test]
    fn test_missing_parse_test_listing() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                )
            "#
            ),
            "Missing required parameter",
        );
        Ok(())
    }

    #[test]
    fn test_missing_parse_test_result() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    listing_command = ["binary", "--list"],
                )
            "#
            ),
            "Missing required parameter",
        );
        Ok(())
    }

    // ---------------------------------------------------------------------------
    // Validation: listing_command
    // ---------------------------------------------------------------------------

    #[test]
    fn test_missing_listing_command() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                )
            "#
            ),
            "Missing required parameter",
        );
        Ok(())
    }

    #[test]
    fn test_listing_command_empty_validation() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = [],
                )
            "#
            ),
            "`listing_command` must be non-empty",
        );
        Ok(())
    }

    #[test]
    fn test_listing_command_with_cmd_args() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test(indoc!(
            r#"
        def test():
            InternalRunnerTestInfo(
                type = "custom",
                parse_test_listing = lambda stdout: [],
                parse_test_result = lambda stdout, stderr, exit_code: [],
                listing_command = ["binary", cmd_args(), "--list"],
            )
        "#
        ))?;
        Ok(())
    }

    // ---------------------------------------------------------------------------
    // Validation: wrong types for required parameters
    // ---------------------------------------------------------------------------

    #[test]
    fn test_type_must_be_string() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = 123,
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                )
            "#
            ),
            "Expected type `str` but got `int`",
        );
        Ok(())
    }

    #[test]
    fn test_parse_test_listing_must_be_callable() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = "not_a_function",
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                )
            "#
            ),
            "Expected type `typing.Callable[[str], list[dict[str, typing.Any]]]` but got `str`",
        );
        Ok(())
    }

    #[test]
    fn test_parse_test_result_must_be_callable() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = "not_a_function",
                    listing_command = ["binary", "--list"],
                )
            "#
            ),
            "Expected type `typing.Callable[[str, str, int], list[dict[str, typing.Any]]]` but got `str`",
        );
        Ok(())
    }

    // ---------------------------------------------------------------------------
    // Validation: wrong types for optional parameters
    // ---------------------------------------------------------------------------

    #[test]
    fn test_command_validation() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                    command = "not_a_list",
                )
            "#
            ),
            "Expected type `None | list[Artifact",
        );

        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                    command = [123],
                )
            "#
            ),
            "but got `list[int]`",
        );
        Ok(())
    }

    #[test]
    fn test_env_validation() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                    env = "not_a_dict",
                )
            "#
            ),
            "Expected type `None | dict[str, Artifact",
        );

        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                    env = {"key": 123},
                )
            "#
            ),
            "but got `dict[str, int]`",
        );
        Ok(())
    }

    #[test]
    fn test_labels_validation() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                    labels = "not_a_list",
                )
            "#
            ),
            "Expected type `None | list[str] | tuple[str, ...]` but got `str`",
        );

        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                    labels = [123],
                )
            "#
            ),
            "but got `list[int]`",
        );
        Ok(())
    }

    #[test]
    fn test_contacts_validation() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                    contacts = "not_a_list",
                )
            "#
            ),
            "Expected type `None | list[str] | tuple[str, ...]` but got `str`",
        );

        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                    contacts = [123],
                )
            "#
            ),
            "but got `list[int]`",
        );
        Ok(())
    }

    #[test]
    fn test_bool_field_validation() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                    use_project_relative_paths = "yes",
                )
            "#
            ),
            "Expected type `None | bool` but got `str`",
        );

        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                    run_from_project_root = "yes",
                )
            "#
            ),
            "Expected type `None | bool` but got `str`",
        );
        Ok(())
    }

    #[test]
    fn test_executor_overrides_validation() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                    executor_overrides = {"foo": "not_an_executor"},
                )
            "#
            ),
            "Expected type `None | dict[str, CommandExecutorConfig]` but got `dict[str, str]`",
        );
        Ok(())
    }

    #[test]
    fn test_default_executor_validation() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                    default_executor = "not_an_executor",
                )
            "#
            ),
            "Expected type `CommandExecutorConfig | None` but got `str`",
        );
        Ok(())
    }

    #[test]
    fn test_required_local_resources_validation() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                    required_local_resources = ["not_a_resource"],
                )
            "#
            ),
            "Expected type `None | list[RequiredTestLocalResource] | tuple[RequiredTestLocalResource, ...]` but got `list[str]`",
        );

        tester.run_starlark_bzl_test_expecting_error(
            indoc!(
                r#"
            def test():
                InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                    required_local_resources = [RequiredTestLocalResource("gpu")],
                )
            "#
            ),
            "`required_local_resources` contains `gpu` which is not present in `local_resources`",
        );
        Ok(())
    }

    // ---------------------------------------------------------------------------
    // Freeze-time validation: mutation after construction
    // ---------------------------------------------------------------------------

    #[test]
    fn test_validation_at_freeze_contacts_mutation() -> buck2_error::Result<()> {
        let mut tester = tester();
        let res = tester.add_import(
            &ImportPath::testing_new("root//test:def1.bzl"),
            indoc!(
                r#"
            def make_info():
                contacts = []
                info = InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                    contacts = contacts,
                )
                contacts.append(123)
                return info

            exported_info = make_info()
            "#
            ),
        );
        assert!(res.is_err());
        Ok(())
    }

    #[test]
    fn test_validation_at_freeze_labels_mutation() -> buck2_error::Result<()> {
        let mut tester = tester();
        let res = tester.add_import(
            &ImportPath::testing_new("root//test:def2.bzl"),
            indoc!(
                r#"
            def make_info():
                labels = []
                info = InternalRunnerTestInfo(
                    type = "custom",
                    parse_test_listing = lambda stdout: [],
                    parse_test_result = lambda stdout, stderr, exit_code: [],
                    listing_command = ["binary", "--list"],
                    labels = labels,
                )
                labels.append(123)
                return info

            exported_info = make_info()
            "#
            ),
        );
        assert!(res.is_err());
        Ok(())
    }

    // ---------------------------------------------------------------------------
    // parse_test_listing callback: functional behavior via Starlark
    // ---------------------------------------------------------------------------

    #[test]
    fn test_parse_test_listing_callback_returns_list() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test(indoc!(
            r#"
        def my_listing_parser(stdout):
            results = []
            for line in stdout.strip().split("\n"):
                if line:
                    results.append({"name": line, "filter": line})
            return results

        def my_result_parser(stdout, stderr, exit_code):
            return []

        def test():
            info = InternalRunnerTestInfo(
                type = "custom",
                parse_test_listing = my_listing_parser,
                parse_test_result = my_result_parser,
                listing_command = ["binary", "--list"],
            )
            # Verify the provider can be constructed with a real function
            assert_true(isinstance(info, InternalRunnerTestInfo))
        "#
        ))?;
        Ok(())
    }

    #[test]
    fn test_parse_test_listing_with_name_and_filter() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test(indoc!(
            r#"
        def my_listing_parser(stdout):
            return [
                {
                    "name": "test_foo",
                    "filter": "module.TestClass.test_foo",
                },
            ]

        def noop_result(stdout, stderr, exit_code):
            return []

        def test():
            info = InternalRunnerTestInfo(
                type = "python",
                parse_test_listing = my_listing_parser,
                parse_test_result = noop_result,
                listing_command = ["binary", "--list"],
            )
            assert_true(isinstance(info, InternalRunnerTestInfo))
        "#
        ))?;
        Ok(())
    }

    // ---------------------------------------------------------------------------
    // parse_test_result callback: functional behavior via Starlark
    // ---------------------------------------------------------------------------

    #[test]
    fn test_parse_test_result_callback_returns_list() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test(indoc!(
            r#"
        def my_result_parser(stdout, stderr, exit_code):
            results = []
            if exit_code == 0:
                results.append({
                    "name": "test_all",
                    "status": "PASS",
                })
            else:
                results.append({
                    "name": "test_all",
                    "status": "FAIL",
                    "message": "Non-zero exit code: " + str(exit_code),
                })
            return results

        def noop_listing(stdout):
            return []

        def test():
            info = InternalRunnerTestInfo(
                type = "custom",
                parse_test_listing = noop_listing,
                parse_test_result = my_result_parser,
                listing_command = ["binary", "--list"],
            )
            assert_true(isinstance(info, InternalRunnerTestInfo))
        "#
        ))?;
        Ok(())
    }

    #[test]
    fn test_parse_test_result_with_all_fields() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test(indoc!(
            r#"
        def my_result_parser(stdout, stderr, exit_code):
            return [
                {
                    "name": "test_addition",
                    "status": "PASS",
                    "message": None,
                    "duration": 0.123,
                    "details": "All assertions passed",
                },
                {
                    "name": "test_subtraction",
                    "status": "FAIL",
                    "message": "Expected 3, got 4",
                    "duration": 1.5,
                    "details": stderr,
                },
            ]

        def noop_listing(stdout):
            return []

        def test():
            info = InternalRunnerTestInfo(
                type = "custom",
                parse_test_listing = noop_listing,
                parse_test_result = my_result_parser,
                listing_command = ["binary", "--list"],
            )
            assert_true(isinstance(info, InternalRunnerTestInfo))
        "#
        ))?;
        Ok(())
    }

    #[test]
    fn test_parse_test_result_with_integer_duration() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test(indoc!(
            r#"
        def my_result_parser(stdout, stderr, exit_code):
            return [
                {
                    "name": "test_slow",
                    "status": "PASS",
                    "duration": 5,
                },
            ]

        def noop_listing(stdout):
            return []

        def test():
            info = InternalRunnerTestInfo(
                type = "custom",
                parse_test_listing = noop_listing,
                parse_test_result = my_result_parser,
                listing_command = ["binary", "--list"],
            )
            assert_true(isinstance(info, InternalRunnerTestInfo))
        "#
        ))?;
        Ok(())
    }

    // ---------------------------------------------------------------------------
    // Realistic end-to-end callback: JSON-based test runner
    // ---------------------------------------------------------------------------

    #[test]
    fn test_json_based_listing_and_result_parsers() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test(indoc!(
            r#"
        def parse_json_listing(stdout):
            tests = json.decode(stdout)
            results = []
            for t in tests:
                results.append({
                    "name": t["name"],
                    "filter": t.get("filter", t["name"]),
                })
            return results

        def parse_json_result(stdout, stderr, exit_code):
            results = json.decode(stdout)
            entries = []
            for r in results:
                entries.append({
                    "name": r["name"],
                    "status": r["status"],
                    "message": r.get("message", None),
                    "duration": r.get("duration", None),
                    "details": r.get("details", None),
                })
            return entries

        def test():
            info = InternalRunnerTestInfo(
                type = "json_runner",
                parse_test_listing = parse_json_listing,
                parse_test_result = parse_json_result,
                listing_command = ["binary", "--list"],
            )
            assert_true(isinstance(info, InternalRunnerTestInfo))
        "#
        ))?;
        Ok(())
    }

    // ---------------------------------------------------------------------------
    // Realistic end-to-end callback: line-based test runner
    // ---------------------------------------------------------------------------

    #[test]
    fn test_line_based_listing_parser() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test(indoc!(
            r#"
        def parse_line_listing(stdout):
            entries = []
            for line in stdout.strip().split("\n"):
                line = line.strip()
                if line:
                    entries.append({"name": line, "filter": line})
            return entries

        def parse_exit_code_result(stdout, stderr, exit_code):
            if exit_code == 0:
                return [{"name": "suite", "status": "PASS"}]
            else:
                return [{"name": "suite", "status": "FAIL", "message": stderr}]

        def test():
            info = InternalRunnerTestInfo(
                type = "line_runner",
                parse_test_listing = parse_line_listing,
                parse_test_result = parse_exit_code_result,
                listing_command = ["run_tests", "--list"],
                command = ["run_tests"],
            )
            assert_true(isinstance(info, InternalRunnerTestInfo))
        "#
        ))?;
        Ok(())
    }

    // ---------------------------------------------------------------------------
    // Provider identity: isinstance checks
    // ---------------------------------------------------------------------------

    #[test]
    fn test_isinstance_checks() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test(indoc!(
            r#"
        def test():
            info = InternalRunnerTestInfo(
                type = "custom",
                parse_test_listing = lambda stdout: [],
                parse_test_result = lambda stdout, stderr, exit_code: [],
                listing_command = ["binary", "--list"],
            )
            assert_true(isinstance(info, InternalRunnerTestInfo))
            assert_true(isinstance(info, Provider))
            assert_false(isinstance(info, DefaultInfo))
            assert_false(isinstance(info, ExternalRunnerTestInfo))
        "#
        ))?;
        Ok(())
    }

    // ---------------------------------------------------------------------------
    // Multiple status values in parse_test_result
    // ---------------------------------------------------------------------------

    #[test]
    fn test_parse_test_result_various_statuses() -> buck2_error::Result<()> {
        let mut tester = tester();
        tester.run_starlark_bzl_test(indoc!(
            r#"
        def my_result_parser(stdout, stderr, exit_code):
            return [
                {"name": "test_pass", "status": "PASS"},
                {"name": "test_fail", "status": "FAIL", "message": "assertion error"},
                {"name": "test_skip", "status": "SKIP", "message": "not applicable"},
                {"name": "test_timeout", "status": "TIMEOUT"},
                {"name": "test_omitted", "status": "OMITTED"},
                {"name": "test_fatal", "status": "FATAL", "message": "crash"},
                {"name": "test_listing_success", "status": "LISTING_SUCCESS"},
                {"name": "test_listing_fail", "status": "LISTING_FAILED"},
            ]

        def noop_listing(stdout):
            return []

        def test():
            info = InternalRunnerTestInfo(
                type = "custom",
                parse_test_listing = noop_listing,
                parse_test_result = my_result_parser,
                listing_command = ["binary", "--list"],
            )
            assert_true(isinstance(info, InternalRunnerTestInfo))
        "#
        ))?;
        Ok(())
    }

    // ---------------------------------------------------------------------------
    // Rust-side callback invocation: parse_test_listing_output
    // ---------------------------------------------------------------------------

    #[test]
    fn test_parse_test_listing_output_basic() -> buck2_error::Result<()> {
        let info = freeze_provider(indoc!(
            r#"
        def my_listing(stdout):
            results = []
            for line in stdout.strip().split("\n"):
                if line:
                    results.append({"name": line, "filter": line})
            return results

        exported_info = InternalRunnerTestInfo(
            type = "custom",
            parse_test_listing = my_listing,
            parse_test_result = lambda stdout, stderr, exit_code: [],
            listing_command = ["binary", "--list"],
        )
        "#
        ))?;

        let entries =
            InternalRunnerTestInfo::parse_test_listing_output(&info, "test_foo\ntest_bar\n")?;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "test_foo");
        assert_eq!(entries[0].filter, "test_foo");
        assert_eq!(entries[1].name, "test_bar");
        assert_eq!(entries[1].filter, "test_bar");
        Ok(())
    }

    #[test]
    fn test_parse_test_listing_output_empty() -> buck2_error::Result<()> {
        let info = freeze_provider(indoc!(
            r#"
        exported_info = InternalRunnerTestInfo(
            type = "custom",
            parse_test_listing = lambda stdout: [],
            parse_test_result = lambda stdout, stderr, exit_code: [],
            listing_command = ["binary", "--list"],
        )
        "#
        ))?;

        let entries = InternalRunnerTestInfo::parse_test_listing_output(&info, "")?;
        assert_eq!(entries.len(), 0);
        Ok(())
    }

    #[test]
    fn test_parse_test_listing_output_name_differs_from_filter() -> buck2_error::Result<()> {
        let info = freeze_provider(indoc!(
            r#"
        def my_listing(stdout):
            return [
                {"name": "Test Addition", "filter": "math::TestAddition"},
                {"name": "Test Subtraction", "filter": "math::TestSubtraction"},
            ]

        exported_info = InternalRunnerTestInfo(
            type = "custom",
            parse_test_listing = my_listing,
            parse_test_result = lambda stdout, stderr, exit_code: [],
            listing_command = ["binary", "--list"],
        )
        "#
        ))?;

        let entries = InternalRunnerTestInfo::parse_test_listing_output(&info, "ignored")?;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "Test Addition");
        assert_eq!(entries[0].filter, "math::TestAddition");
        Ok(())
    }

    #[test]
    fn test_parse_test_listing_output_missing_key() -> buck2_error::Result<()> {
        let info = freeze_provider(indoc!(
            r#"
        exported_info = InternalRunnerTestInfo(
            type = "custom",
            parse_test_listing = lambda stdout: [{"name": "test"}],
            parse_test_result = lambda stdout, stderr, exit_code: [],
            listing_command = ["binary", "--list"],
        )
        "#
        ))?;

        let err = InternalRunnerTestInfo::parse_test_listing_output(&info, "x").unwrap_err();
        assert!(err.to_string().contains("missing required key"), "{}", err);
        Ok(())
    }

    // ---------------------------------------------------------------------------
    // Rust-side callback invocation: parse_test_result_output
    // ---------------------------------------------------------------------------

    #[test]
    fn test_parse_test_result_output_basic() -> buck2_error::Result<()> {
        let info = freeze_provider(indoc!(
            r#"
        def my_result(stdout, stderr, exit_code):
            if exit_code == 0:
                return [{"name": "test_all", "status": "PASS"}]
            else:
                return [{"name": "test_all", "status": "FAIL", "message": stderr}]

        exported_info = InternalRunnerTestInfo(
            type = "custom",
            parse_test_listing = lambda stdout: [],
            parse_test_result = my_result,
            listing_command = ["binary", "--list"],
        )
        "#
        ))?;

        let results = InternalRunnerTestInfo::parse_test_result_output(&info, "", "", 0)?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "test_all");
        assert_eq!(results[0].status, TestStatus::PASS);
        assert!(results[0].message.is_none());

        let results = InternalRunnerTestInfo::parse_test_result_output(&info, "", "oops", 1)?;
        assert_eq!(results[0].status, TestStatus::FAIL);
        assert_eq!(results[0].message.as_deref(), Some("oops"));
        Ok(())
    }

    #[test]
    fn test_parse_test_result_output_all_fields() -> buck2_error::Result<()> {
        let info = freeze_provider(indoc!(
            r#"
        def my_result(stdout, stderr, exit_code):
            return [{
                "name": "test_math",
                "status": "FAIL",
                "message": "Expected 3, got 4",
                "details": "full stack trace here",
                "duration": 1.5,
            }]

        exported_info = InternalRunnerTestInfo(
            type = "custom",
            parse_test_listing = lambda stdout: [],
            parse_test_result = my_result,
            listing_command = ["binary", "--list"],
        )
        "#
        ))?;

        let results = InternalRunnerTestInfo::parse_test_result_output(&info, "", "", 1)?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "test_math");
        assert_eq!(results[0].status, TestStatus::FAIL);
        assert_eq!(results[0].message.as_deref(), Some("Expected 3, got 4"));
        assert_eq!(results[0].details.as_deref(), Some("full stack trace here"));
        assert_eq!(
            results[0].duration,
            Some(std::time::Duration::from_millis(1500))
        );
        Ok(())
    }

    #[test]
    fn test_parse_test_result_output_integer_duration() -> buck2_error::Result<()> {
        let info = freeze_provider(indoc!(
            r#"
        def my_result(stdout, stderr, exit_code):
            return [{"name": "test_slow", "status": "PASS", "duration": 5}]

        exported_info = InternalRunnerTestInfo(
            type = "custom",
            parse_test_listing = lambda stdout: [],
            parse_test_result = my_result,
            listing_command = ["binary", "--list"],
        )
        "#
        ))?;

        let results = InternalRunnerTestInfo::parse_test_result_output(&info, "", "", 0)?;
        assert_eq!(results[0].duration, Some(std::time::Duration::from_secs(5)));
        Ok(())
    }

    #[test]
    fn test_parse_test_result_output_negative_duration() -> buck2_error::Result<()> {
        let info = freeze_provider(indoc!(
            r#"
        def my_result(stdout, stderr, exit_code):
            return [{"name": "test", "status": "PASS", "duration": -1.0}]

        exported_info = InternalRunnerTestInfo(
            type = "custom",
            parse_test_listing = lambda stdout: [],
            parse_test_result = my_result,
            listing_command = ["binary", "--list"],
        )
        "#
        ))?;

        let err = InternalRunnerTestInfo::parse_test_result_output(&info, "", "", 0).unwrap_err();
        assert!(err.to_string().contains("non-negative"), "{}", err);
        Ok(())
    }

    #[test]
    fn test_parse_test_result_output_invalid_status() -> buck2_error::Result<()> {
        let info = freeze_provider(indoc!(
            r#"
        def my_result(stdout, stderr, exit_code):
            return [{"name": "test", "status": "BOGUS"}]

        exported_info = InternalRunnerTestInfo(
            type = "custom",
            parse_test_listing = lambda stdout: [],
            parse_test_result = my_result,
            listing_command = ["binary", "--list"],
        )
        "#
        ))?;

        let err = InternalRunnerTestInfo::parse_test_result_output(&info, "", "", 0).unwrap_err();
        assert!(err.to_string().contains("Unknown test status"), "{}", err);
        Ok(())
    }

    #[test]
    fn test_parse_test_result_output_missing_name() -> buck2_error::Result<()> {
        let info = freeze_provider(indoc!(
            r#"
        def my_result(stdout, stderr, exit_code):
            return [{"status": "PASS"}]

        exported_info = InternalRunnerTestInfo(
            type = "custom",
            parse_test_listing = lambda stdout: [],
            parse_test_result = my_result,
            listing_command = ["binary", "--list"],
        )
        "#
        ))?;

        let err = InternalRunnerTestInfo::parse_test_result_output(&info, "", "", 0).unwrap_err();
        assert!(err.to_string().contains("missing required key"), "{}", err);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Rust-side: listing_command accessor
// ---------------------------------------------------------------------------

#[test]
fn test_listing_command_accessor() -> buck2_error::Result<()> {
    use buck2_build_api::interpreter::rule_defs::provider::builtin::external_runner_test_info::TestCommandMember;

    let info = freeze_provider(indoc!(
        r#"
        exported_info = InternalRunnerTestInfo(
            type = "gtest",
            parse_test_listing = lambda stdout: [],
            parse_test_result = lambda stdout, stderr, exit_code: [],
            listing_command = ["my_binary", "--gtest_list_tests"],
            command = ["my_binary"],
        )
        "#
    ))?;

    let listing_cmd: Vec<String> = info
        .as_ref()
        .value()
        .as_ref()
        .listing_command()
        .map(|m| {
            Ok(match m? {
                TestCommandMember::Literal(s) => s.to_owned(),
                _ => "<arg>".to_owned(),
            })
        })
        .collect::<buck2_error::Result<_>>()?;
    assert_eq!(listing_cmd, vec!["my_binary", "--gtest_list_tests"]);

    let exec_cmd: Vec<String> = info
        .as_ref()
        .value()
        .as_ref()
        .command()
        .map(|m| {
            Ok(match m? {
                TestCommandMember::Literal(s) => s.to_owned(),
                _ => "<arg>".to_owned(),
            })
        })
        .collect::<buck2_error::Result<_>>()?;
    assert_eq!(exec_cmd, vec!["my_binary"]);
    Ok(())
}
