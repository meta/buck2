# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import re

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.api.buck_result import BuckException
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase
from buck2.tests.e2e_util.helper.golden import golden

# One target per character a `superconsole::Span` refuses, each reporting a single
# test case under a name that holds it (see `TARGETS.fixture`).
TARGETS = ["line_break", "tab"]

# `line_break` reports a failing test and `tab` a skipped one.
EXPECTED_EXIT_CODES = {"line_break": 32, "tab": 0}

_TIMESTAMP = re.compile(r"^\[[^\]]+\] ?")
_ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")
_RESULT_LINE = re.compile(r"^(✗ Fail|↷ Skip): ")
_SPAN_ERROR = "contains non-space whitespace"


async def _test(buck: Buck, console: str, target: str) -> tuple[int, str]:
    """Runs `buck2 test` and returns its exit code and stderr, without raising."""
    try:
        result = await buck.test("--console", console, f":{target}")
        return 0, result.stderr
    except BuckException as e:
        exit_code = e.process.returncode
        assert exit_code is not None, "a failed buck2 process has exited"
        return exit_code, e.stderr


def _simple_console_output(exit_code: int, stderr: str) -> str:
    """The exit code, the test result lines, and any command failure, as shown."""
    lines = [f"exit code: {exit_code}"]
    in_failure = False
    for line in stderr.splitlines():
        line = _TIMESTAMP.sub("", line)
        in_failure = in_failure or line.startswith("Command failed:")
        if line and (in_failure or _RESULT_LINE.match(line)):
            lines.append(line)
    return "\n".join(lines)


class TestNameWhitespaceTest(BuckTestCase):
    @buck_test()
    async def test_simple_console(self, buck: Buck) -> None:
        for target in TARGETS:
            exit_code, stderr = await _test(buck, "simple", target)
            golden(
                output=_simple_console_output(exit_code, stderr),
                rel_path=f"fixtures/{target}.simple.golden.txt",
            )

    @buck_test()
    async def test_superconsole(self, buck: Buck) -> None:
        # The superconsole redraws its output, so the test checks the outcome
        # rather than comparing it with a golden file.
        for target in TARGETS:
            exit_code, stderr = await _test(buck, "super", target)
            stderr = _ANSI_ESCAPE.sub("", stderr)
            assert _SPAN_ERROR not in stderr, f"{target}: {stderr}"
            assert any(_RESULT_LINE.match(line) for line in stderr.splitlines()), (
                f"{target}: no result line in {stderr}"
            )
            assert exit_code == EXPECTED_EXIT_CODES[target], (
                f"{target}: exit code {exit_code}"
            )
