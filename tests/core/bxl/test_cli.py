# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


import json
import os
import re

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase
from buck2.tests.e2e_util.helper.golden import golden, GOLDEN_DIRECTORY, sanitize_hashes


def _extract_configuration(s: str) -> list[str]:
    return re.findall(r"\((<modifiers>#[a-f0-9]+)\)", s)


class CliTest(BuckTestCase):
    @buck_test()
    async def test_bxl_cli(self, buck: Buck) -> None:
        result = await buck.bxl(
            "//cli_args.bxl:cli_test",
            "--",
            "--int_arg",
            "1",
            "--float_arg",
            "4.3",
            "--enum_type",
            "a",
            "--list_type",
            "1",
            "2",
            "3",
            "--target",
            ":foo",
            "--sub_target",
            "cell/pkg:bar[sub]",
            "--configured_target",
            "root//:t1?root//:macos",
        )

        golden(
            output=sanitize_hashes(result.stdout),
            rel_path=GOLDEN_DIRECTORY + "test_bxl_cli_standard.golden.txt",
        )

        result = await buck.bxl(
            "//cli_args.bxl:cli_test",
            "--",
            # Override default bool arg with false
            "--bool_arg_with_default",
            "false",
            "--int_arg",
            "2",
            "--float_arg",
            "3.4",
            "--optional",
            "value",
            "--enum_type",
            "b",
            "--list_type",
            "1",
            "--target",
            "bar:foo",
            "--sub_target",
            "cell/pkg:bar",
            "--configured_target",
            "root//:t1?root//:macos",
        )

        golden(
            output=sanitize_hashes(result.stdout),
            rel_path=GOLDEN_DIRECTORY + "test_bxl_cli_override_default.golden.txt",
        )

        # multiple occurrences of a list-type argument
        # i.e., --arg 1 --arg 2 --arg 3
        result = await buck.bxl(
            "//cli_args.bxl:cli_test",
            "--",
            "--int_arg",
            "1",
            "--float_arg",
            "4.3",
            "--enum_type",
            "a",
            "--list_type",
            "1",
            "--list_type",
            "2",
            "--list_type",
            "3",
            "--target",
            ":foo",
            "--sub_target",
            "cell/pkg:bar[sub]",
            "--configured_target",
            "root//:t1?root//:macos",
        )

        golden(
            output=sanitize_hashes(result.stdout),
            rel_path=GOLDEN_DIRECTORY
            + "test_bxl_cli_multiple_list_occurrences.golden.txt",
        )

        # illegal target
        await expect_failure(
            buck.bxl(
                "//cli_args.bxl:cli_test",
                "--",
                "--int_arg",
                "2",
                "--float_arg",
                "3.4",
                "--optional",
                "value",
                "--enum_type",
                "b",
                "--list_type",
                "1",
                "--target",
                "illegal;target",
                "--sub_target",
                "cell/pkg:bar",
                "--configured_target",
                "root//:t1?root//:macos",
            ),
            stderr_regex="Invalid target name `illegal;target`",
        )

        # not int
        await expect_failure(
            buck.bxl(
                "//cli_args.bxl:cli_test",
                "--",
                "--int_arg",
                "2.0",
                "--float_arg",
                "3.4",
                "--optional",
                "value",
                "--enum_type",
                "b",
                "--list_type",
                "1",
                "--target",
                ":foo",
                "--sub_target",
                "cell/pkg:bar",
                "--configured_target",
                "root//:t1?root//:macos",
            ),
            stderr_regex="invalid value '2.0' for '--int_arg <int_arg>': invalid digit found in string",
        )

        # list inner type mismatch
        await expect_failure(
            buck.bxl(
                "//cli_args.bxl:cli_test",
                "--",
                "--int_arg",
                "2",
                "--float_arg",
                "3.4",
                "--optional",
                "value",
                "--enum_type",
                "b",
                "--list_type",
                "wrong_inner_list_type",
                "--target",
                "bar:foo",
                "--sub_target",
                "cell/pkg:bar",
                "--configured_target",
                "root//:t1?root//:macos",
            ),
            stderr_regex=r"invalid value 'wrong_inner_list_type' for '--list_type \[<list_type>...\]': invalid digit found in string",
        )

        # not valid enum variant
        await expect_failure(
            buck.bxl(
                "//cli_args.bxl:cli_test",
                "--",
                "--int_arg",
                "2",
                "--float_arg",
                "3.4",
                "--optional",
                "value",
                "--enum_type",
                "not_enum",
                "--list_type",
                "1",
                "--target",
                ":foo",
                "--sub_target",
                "cell/pkg:bar",
                "--configured_target",
                "root//:t1?root//:macos",
            ),
            stderr_regex="invalid value 'not_enum' for '--enum_type <enum_type>'",
        )

        # missing non-optional field
        await expect_failure(
            buck.bxl(
                "//cli_args.bxl:cli_test",
                "--",
                "--int_arg",
                "2",
                "--optional",
                "value",
                "--enum_type",
                "a",
                "--list_type",
                "1",
                "--target",
                ":foo",
                "--sub_target",
                "cell/pkg:bar",
                "--configured_target",
                "root//:t1?root//:macos",
            ),
            stderr_regex="Missing cli arg from command line that isn't optional nor has any default values",
        )

        # check short args work
        result = await buck.bxl(
            "//cli_args.bxl:cli_test_short",
            "--",
            "-i",
            "1",
            "-f",
            "4.3",
            "-e",
            "a",
            "-l",
            "1",
            "2",
            "3",
            "-t",
            ":foo",
            "-s",
            "default",
            "-c",
            "root//:t1?root//:macos",
        )

        golden(
            output=sanitize_hashes(result.stdout),
            rel_path=GOLDEN_DIRECTORY + "test_bxl_cli_short.golden.txt",
        )

        # check long args still work with short args
        result = await buck.bxl(
            "//cli_args.bxl:cli_test_short",
            "--",
            "--int_arg",
            "1",
            "--float_arg",
            "4.3",
            "--enum_type",
            "a",
            "--list_type",
            "1",
            "2",
            "3",
            "--target",
            ":foo",
            "--string_arg",
            "default",
            "--configured_target",
            "root//:t1?root//:macos",
        )

        golden(
            output=sanitize_hashes(result.stdout),
            rel_path=GOLDEN_DIRECTORY + "test_bxl_cli_short_long.golden.txt",
        )

        # check snakecase cli_arg access from bxl context, make sure it still works with default args and shorthand args
        result = await buck.bxl(
            "//cli_args.bxl:cli_test_snakecase_access",
            "--",
            "--my-arg",
            "this is my arg",
        )
        assert result.stdout == 'my-arg: "this is my arg"\n'

        result = await buck.bxl(
            "//cli_args.bxl:cli_test_snakecase_access", "--", "-a", "this is my arg"
        )
        assert result.stdout == 'my-arg: "this is my arg"\n'

        result = await buck.bxl("//cli_args.bxl:cli_test_snakecase_access")
        assert result.stdout == 'my-arg: "default"\n'

        await expect_failure(
            buck.bxl(
                "//cli_args_bad_case.bxl:cli_test_bad_case",
                "--",
                "--my-arg",
                "this is my arg",
            )
        )

    @buck_test()
    async def test_bxl_cli_json_args(self, buck: Buck) -> None:
        json_args = {
            "int": 1,
            "string": "foo",
            "float": 1.0,
            "bool": True,
            "none": None,
            "list": [1, 2, 3],
            "nested": {"nested_string": "bar", "nested_int": -1},
        }

        my_json = json.dumps(json_args)

        await buck.bxl(
            "//cli_args.bxl:cli_json_arg",
            "--",
            "--my-json",
            my_json,
        )

        await expect_failure(
            buck.bxl(
                "//cli_args.bxl:cli_json_arg",
                "--",
                "--my-json",
                "[1,2,3]",
            ),
            stderr_regex="Expecting json object. Got: `\\[1,2,3]\\`",
        )

    @buck_test()
    async def test_bxl_cli_short_bad(self, buck: Buck) -> None:
        # duplicate "short"
        await expect_failure(
            buck.bxl(
                "//cli_args_bad.bxl:cli_test_short_bad",
                "--",
                "-a",
                "2",
                "-a",
                "value",
                "-a",
                "a",
                "-a",
                "1",
                "-a",
                ":foo",
                "-a",
                "default",
            ),
            stderr_regex="Duplicate short args are not allowed",
        )

    @buck_test()
    async def test_cli_target_pattern(self, buck: Buck) -> None:
        result = await buck.bxl(
            "//cli_args.bxl:target_expr_test",
            "--",
            "--targets",
            ":t1",
        )
        assert "[root//:t1]" in result.stdout

        result = await buck.bxl(
            "//cli_args.bxl:target_expr_test",
            "--",
            "--targets",
            "root//:",
        )
        assert "root//:t1" in result.stdout
        assert "root//:t2" in result.stdout

        await expect_failure(
            buck.bxl(
                "//cli_args.bxl:target_expr_test",
                "--",
                "--targets",
                ":non-existent",
            ),
            stderr_regex="Unknown target `non-existent` from package `root//`",
        )

        await expect_failure(
            buck.bxl(
                "//cli_args.bxl:target_expr_test",
                "--",
                "--targets",
                "invalid/...",
            ),
            stderr_regex="Directory `invalid` does not exist",
        )

    @buck_test()
    async def test_cli_sub_target_pattern(self, buck: Buck) -> None:
        # Tests where no sub-target is specified; should ensure functionality
        # of regular target patterns work with these subtarget patterns.
        result = await buck.bxl(
            "//cli_args.bxl:sub_target_expr_test",
            "--",
            "--sub_targets",
            ":t1",
        )
        assert "[root//:t1]" in result.stdout

        result = await buck.bxl(
            "//cli_args.bxl:sub_target_expr_test",
            "--",
            "--sub_targets",
            "root//:",
        )
        assert "root//:t1" in result.stdout
        assert "root//:t2" in result.stdout

        # Test single sub-targets.
        result = await buck.bxl(
            "//cli_args.bxl:sub_target_expr_test",
            "--",
            "--sub_targets",
            "root//:t1[sub]",
        )
        assert "[root//:t1[sub]]" in result.stdout

        # Several subtargets / nested subtargets.
        result = await buck.bxl(
            "//cli_args.bxl:sub_target_expr_test",
            "--",
            "--sub_targets",
            "root//:t2[sub1][sub2]",
        )
        assert "[root//:t2[sub1][sub2]]" in result.stdout

        await expect_failure(
            buck.bxl(
                "//cli_args.bxl:sub_target_expr_test",
                "--",
                "--sub_targets",
                ":fake_bin[sub]",
            ),
            stderr_regex="Unknown target `fake_bin` from package `root//`",
        )

    @buck_test()
    async def test_cli_target_fails_with_question_mark_modifier_syntax(
        self, buck: Buck
    ) -> None:
        await expect_failure(
            buck.bxl(
                "//cli_args.bxl:cli_test",
                "--",
                "--int_arg",
                "1",
                "--float_arg",
                "4.3",
                "--enum_type",
                "a",
                "--list_type",
                "1",
                "2",
                "3",
                "--target",
                "root//:t1?root//:macos",
                "--sub_target",
                "cell/pkg:bar[sub]",
                "--configured_target",
                "root//:t1?root//:macos",
            ),
            stderr_regex=r"The \?modifier syntax is unsupported for this command",
        )

        await expect_failure(
            buck.bxl(
                "//cli_args.bxl:target_expr_test",
                "--",
                "--targets",
                "root//:t1?root//:macos",
            ),
            stderr_regex=r"The \?modifier syntax is unsupported for this command",
        )

    @buck_test()
    async def test_cli_configured_target_fails_with_global_modifiers(
        self, buck: Buck
    ) -> None:
        await expect_failure(
            buck.bxl(
                "--modifier",
                "root//:macos",
                "//cli_args.bxl:cli_configured_target",
                "--",
                "--configured_target",
                "root//:t1?root//:macos",
            ),
            stderr_regex=r"Cannot specify modifiers with \?modifier syntax when global CLI modifiers are set with --modifier flag",
        )

        await expect_failure(
            buck.bxl(
                "--modifier",
                "root//:macos",
                "//cli_args.bxl:cli_configured_target",
                "--",
                "--configured_target",
                "root//:?root//:macos",
            ),
            stderr_regex=r"Cannot specify modifiers with \?modifier syntax when global CLI modifiers are set with --modifier flag",
        )

    @buck_test()
    async def test_cli_configured_target_pattern(self, buck: Buck) -> None:
        result = await buck.bxl(
            "//cli_args.bxl:cli_configured_target",
            "--",
            "--configured_target",
            "root//:t1?root//:macos",
        )

        [configuration] = _extract_configuration(result.stdout)

        cfg = await buck.audit_configurations(configuration)

        assert "root//:macos" in cfg.stdout

        # test multiple modifiers
        result = await buck.bxl(
            "//cli_args.bxl:cli_configured_target",
            "--",
            "--configured_target",
            "root//:t1?root//:macos+root//:arm",
        )

        [configuration] = _extract_configuration(result.stdout)

        cfg = await buck.audit_configurations(configuration)

        assert "root//:macos" in cfg.stdout
        assert "root//:arm" in cfg.stdout

        # test order of modifiers
        # if passing in modifiers of the same constraint setting,
        # the last one should be the one that applies
        result = await buck.bxl(
            "//cli_args.bxl:cli_configured_target",
            "--",
            "--configured_target",
            "root//:t1?root//:macos+root//:linux",
        )

        [configuration] = _extract_configuration(result.stdout)

        cfg = await buck.audit_configurations(configuration)

        assert "root//:linux" in cfg.stdout
        assert "root//:macos" not in cfg.stdout

        # test no modifiers
        result = await buck.bxl(
            "//cli_args.bxl:cli_configured_target",
            "--",
            "--configured_target",
            "root//:t1",
        )

        assert "configured_target: root//:t1 (<unspecified>)" in result.stdout

        # test expr
        result = await buck.bxl(
            "//cli_args.bxl:cli_configured_target_expr",
            "--",
            "--configured_target_expr",
            "//:t1?root//:macos",
        )

        [configuration] = _extract_configuration(result.stdout)
        cfg = await buck.audit_configurations(configuration)

        assert "root//:macos" in cfg.stdout

        # test expr with package pattern
        result = await buck.bxl(
            "//cli_args.bxl:cli_configured_target_expr",
            "--",
            "--configured_target_expr",
            "root//:?root//:macos",
        )

        for configuration in _extract_configuration(result.stdout):
            cfg = await buck.audit_configurations(configuration)
            assert "root//:macos" in cfg.stdout

        golden(
            output=sanitize_hashes(result.stdout),
            rel_path=GOLDEN_DIRECTORY
            + "test_cli_configured_target_expr_package.golden.txt",
        )

        # test expr with recursive pattern
        result = await buck.bxl(
            "//cli_args.bxl:cli_configured_target_expr",
            "--",
            "--configured_target_expr",
            "root//...?root//:macos+root//:arm",
        )

        for configuration in _extract_configuration(result.stdout):
            cfg = await buck.audit_configurations(configuration)
            assert "root//:macos" in cfg.stdout
            assert "root//:arm" in cfg.stdout

        golden(
            output=sanitize_hashes(result.stdout),
            rel_path=GOLDEN_DIRECTORY
            + "test_cli_configured_target_expr_recursive.golden.txt",
        )

    @buck_test()
    async def test_cli_configured_target_modifiers_flag(self, buck: Buck) -> None:
        # test single modifier
        result = await buck.bxl(
            "--modifier",
            "root//:macos",
            "//cli_args.bxl:cli_configured_target",
            "--",
            "--configured_target",
            "root//:t1",
        )

        [configuration] = _extract_configuration(result.stdout)

        cfg = await buck.audit_configurations(configuration)
        assert "root//:macos" in cfg.stdout

        # test multiple modifiers
        result = await buck.bxl(
            "--modifier",
            "root//:macos",
            "--modifier",
            "root//:arm",
            "//cli_args.bxl:cli_configured_target",
            "--",
            "--configured_target",
            "root//:t1",
        )

        [configuration] = _extract_configuration(result.stdout)
        cfg = await buck.audit_configurations(configuration)
        assert "root//:macos" in cfg.stdout
        assert "root//:arm" in cfg.stdout

        # test expr
        result = await buck.bxl(
            "--modifier",
            "root//:macos",
            "--modifier",
            "root//:arm",
            "//cli_args.bxl:cli_configured_target_expr",
            "--",
            "--configured_target_expr",
            "//:",
        )

        for configuration in _extract_configuration(result.stdout):
            cfg = await buck.audit_configurations(configuration)
            assert "root//:macos" in cfg.stdout
            assert "root//:arm" in cfg.stdout

        golden(
            output=sanitize_hashes(result.stdout),
            rel_path=GOLDEN_DIRECTORY
            + "test_cli_configured_target_modifiers_flag_expr.golden.txt",
        )

    @buck_test()
    async def test_cli_configured_target_platform(self, buck: Buck) -> None:
        result = await buck.bxl(
            "--target-platforms",
            "root//:p",
            "//cli_args.bxl:cli_configured_target",
            "--",
            "--configured_target",
            "root//:t1",
        )

        assert "configured_target: root//:t1 (root//:p#<HASH>)" in sanitize_hashes(
            result.stdout
        )

        result = await buck.bxl(
            "--target-platforms",
            "root//:p",
            "//cli_args.bxl:cli_configured_target_expr",
            "--",
            "--configured_target_expr",
            "//:",
        )

        golden(
            output=sanitize_hashes(result.stdout),
            rel_path=GOLDEN_DIRECTORY
            + "test_cli_configured_target_platform_expr.golden.txt",
        )

    @buck_test()
    async def test_cli_json_file(self, buck: Buck) -> None:
        json_file_path = os.path.join(buck.cwd, "a.json")

        await buck.bxl(
            "//cli_args.bxl:cli_json_file",
            "--",
            "--json-file",
            json_file_path,
        )

        await buck.bxl(
            "//cli_args.bxl:cli_json_file",
            "--",
            "--json-file",
            "a.json",
        )

    @buck_test()
    async def test_bxl_cli_option_default_is_used(self, buck: Buck) -> None:
        # `cli_args.option(T, default = X)` gives X when the flag is absent, and None when no
        # default was declared.
        result = await buck.bxl("//cli_args.bxl:cli_option_default")
        assert "opt_int: 5\n" in result.stdout
        assert 'opt_str: "consumer"\n' in result.stdout
        assert "opt_bool: True\n" in result.stdout
        assert "opt_list: None\n" in result.stdout

        # A given flag wins.
        result = await buck.bxl(
            "//cli_args.bxl:cli_option_default",
            "--",
            "--opt_int",
            "7",
            "--opt_bool",
            "false",
        )
        assert "opt_int: 7\n" in result.stdout
        assert "opt_bool: False\n" in result.stdout

        # A bare `--opt_list` gives [] like a plain list arg does.
        result = await buck.bxl("//cli_args.bxl:cli_option_default", "--", "--opt_list")
        assert "opt_list: []\n" in result.stdout
