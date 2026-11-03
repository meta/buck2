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


class DiceSettingsTest(BuckTestCase):
    # A `dice` section reaches computations through DICE: changing it between
    # commands re-evaluates what depended on it, in the same daemon. The
    # Starlark call stack limit is the vehicle.
    @buck_test()
    async def test_dice_setting_changes_apply_without_restart(self, buck: Buck) -> None:
        await buck.targets(":")

        await expect_failure(
            buck.targets(":", "--setting", "starlark.max_callstack_size=10"),
            stderr_regex="Starlark call stack overflow",
        )

        result = await buck.targets(":", "--setting", "starlark.max_callstack_size=100")
        assert "restarting daemon" not in result.stderr

        # Back to the default, still the same daemon.
        result = await buck.targets(":")
        assert "restarting daemon" not in result.stderr
