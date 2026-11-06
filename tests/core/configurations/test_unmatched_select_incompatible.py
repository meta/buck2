# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase

# The packages under `opt*` set `buck.unmatched_select_incompatible` to this.
MESSAGE = "only builds for windows"
LINUX = ["--target-platforms", "root//:linux_p"]
WINDOWS = ["--target-platforms", "root//:windows_p"]


class UnmatchedSelectIncompatibleTest(BuckTestCase):
    @buck_test()
    async def test_pattern_skips_unmatched_targets(self, buck: Buck) -> None:
        # A target whose select doesn't match, its parent, and the user of such a toolchain.
        await buck.build("//opt/...", *LINUX)
        result = await buck.cquery("//opt/...", *LINUX)
        assert "root//opt:ok " in result.stdout, result.stdout
        for skipped in ["opt:sel", "opt:parent", "opt:uses_tc", "opt:tc"]:
            assert skipped not in result.stdout, result.stdout

    @buck_test()
    async def test_naming_a_skipped_target_errors(self, buck: Buck) -> None:
        await expect_failure(buck.build("//opt:sel", *LINUX), stderr_regex=MESSAGE)
        await expect_failure(buck.build("//opt:parent", *LINUX), stderr_regex="incompatible")
        await expect_failure(buck.build("//opt:uses_tc", *LINUX), stderr_regex="incompatible")

    @buck_test()
    async def test_matching_platform_builds_everything(self, buck: Buck) -> None:
        await buck.build("//opt/...", *WINDOWS)
        result = await buck.cquery("//opt/...", *WINDOWS)
        assert "opt:parent " in result.stdout, result.stdout
        assert "opt:uses_tc " in result.stdout, result.stdout

    @buck_test()
    async def test_target_compatible_with_stays_strict(self, buck: Buck) -> None:
        await expect_failure(
            buck.build("//opt_tcw/...", *LINUX), stderr_regex="conditions matched"
        )

    @buck_test()
    async def test_select_incompatible_in_target_compatible_with(self, buck: Buck) -> None:
        await buck.build("//opt_tcw_inc/...", *LINUX)
        await expect_failure(
            buck.build("//opt_tcw_inc:tcw", *LINUX), stderr_regex="not for linux"
        )

    @buck_test()
    async def test_package_not_opted_in_keeps_the_error(self, buck: Buck) -> None:
        await expect_failure(
            buck.build("//plain/...", *LINUX), stderr_regex="conditions matched"
        )

    # The skip is a soft error, which BUCK2_HARD_ERROR would turn into a failure.
    @buck_test(allow_soft_errors=True)
    async def test_user_outside_the_package_keeps_upstream_behavior(self, buck: Buck) -> None:
        # Upstream skips a target that is incompatible only through a dependency under a
        # pattern, and errors when it is named.
        await buck.build("//other/...", *LINUX)
        await expect_failure(buck.build("//other:uses_opt", *LINUX), stderr_regex="incompatible")

    @buck_test()
    async def test_child_package_inherits_the_setting(self, buck: Buck) -> None:
        await buck.build("//opt/child/...", *LINUX)
        await expect_failure(buck.build("//opt/child:sel", *LINUX), stderr_regex=MESSAGE)

    @buck_test()
    async def test_exec_compatible_with_follows_the_setting(self, buck: Buck) -> None:
        # Unlike `target_compatible_with`, which declares compatibility itself, an unmatched
        # select in `exec_compatible_with` makes the target incompatible like any other attribute.
        await buck.build("//opt_ecw/...", *LINUX)
        await expect_failure(buck.build("//opt_ecw:ecw", *LINUX), stderr_regex=MESSAGE)

    @buck_test()
    async def test_unmatched_exec_dep_rules_out_its_execution_platform(self, buck: Buck) -> None:
        # Linux is registered first, but the tool can't be configured for it there.
        await buck.build("//opt_exec:runner", *LINUX)
        result = await buck.cquery("deps(//opt_exec:runner)", *LINUX)
        tool = [line for line in result.stdout.splitlines() if "opt_exec:tool " in line]
        assert tool and all("windows_p" in line for line in tool), result.stdout

    @buck_test(extra_buck_config={"buck2": {"error_on_dep_only_incompatible": "root//..."}})
    async def test_dependency_only_incompatibility_is_skipped_even_where_it_errors(
        self, buck: Buck
    ) -> None:
        # `error_on_dep_only_incompatible` makes a pattern fail on a target that is incompatible
        # only through a dependency; in an opted-in package it is skipped like the others.
        await buck.build("//opt/...", *LINUX)
        await expect_failure(buck.build("//other/...", *LINUX), stderr_regex="incompatible")
