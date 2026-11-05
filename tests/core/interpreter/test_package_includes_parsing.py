# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase, parametrize


class PackageIncludesParsingTest(BuckTestCase):
    @buck_test()
    @parametrize("value", ["", "src=>//:defs.bzl::my_rule,"])
    async def test_empty_items_are_skipped(self, buck: Buck, value: str) -> None:
        """An empty `package_includes` value and a trailing comma are not parse errors."""
        with open(buck.cwd / ".buckconfig", "a") as buckconfig:
            buckconfig.write(f"[buildfile]\npackage_includes = {value}\n")
        result = await buck.targets("//:")
        assert result.stdout.strip() == "root//:t"

    @buck_test()
    async def test_blanks_around_the_arrow_are_trimmed(self, buck: Buck) -> None:
        """`src =>` applies to the directory `src`, not to a directory named `src `."""
        with open(buck.cwd / ".buckconfig", "a") as buckconfig:
            buckconfig.write(
                "[buildfile]\npackage_includes = src =>//:defs.bzl::alias = my_rule\n"
            )
        result = await buck.targets("//src:")
        assert result.stdout.strip() == "root//src:included"
