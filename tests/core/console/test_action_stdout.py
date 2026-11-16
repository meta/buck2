# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase
from buck2.tests.e2e_util.helper.utils import filter_events


STDOUT_LINE = "hello from action stdout"
STDERR_LINE = "hello from action stderr"


async def _logged_stdout(buck: Buck) -> str:
    actions = await filter_events(
        buck, "Event", "data", "SpanEnd", "data", "ActionExecution"
    )
    return "".join(
        command["details"]["stdout"]
        for action in actions
        for command in action.get("commands", [])
    )


class ActionStdoutTest(BuckTestCase):
    @buck_test(skip_for_os=["windows"])
    async def test_stdout_of_successful_action_is_dropped_by_default(
        self, buck: Buck
    ) -> None:
        res = await buck.build("--console=simplenotty", "-v", "1,stderr", "//:chatty")
        assert STDERR_LINE in res.stderr, res.stderr
        assert STDOUT_LINE not in res.stderr, res.stderr
        assert STDOUT_LINE not in await _logged_stdout(buck)

    @buck_test(skip_for_os=["windows"])
    async def test_verbosity_stdout_prints_stdout(self, buck: Buck) -> None:
        res = await buck.build("--console=simplenotty", "-v", "1,stdout", "//:chatty")
        assert "stdout:" in res.stderr, res.stderr
        assert STDOUT_LINE in res.stderr, res.stderr
        assert STDERR_LINE not in res.stderr, res.stderr
        assert STDOUT_LINE in await _logged_stdout(buck)

    @buck_test(skip_for_os=["windows"])
    async def test_keep_action_stdout_only_logs_stdout(self, buck: Buck) -> None:
        res = await buck.build(
            "--console=simplenotty", "--keep-action-stdout", "//:chatty"
        )
        assert STDOUT_LINE not in res.stderr, res.stderr
        assert STDOUT_LINE in await _logged_stdout(buck)
