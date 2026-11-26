# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import os
import subprocess
import unittest


class MainFunctionExitStatusTest(unittest.TestCase):
    """A `main_function`'s return value is its binary's exit status, as `sys.exit` reads it."""

    def assert_exits(
        self, binary_env_var: str, behaviour: str, status: int, stderr: str = ""
    ) -> None:
        with self.subTest(binary=binary_env_var, behaviour=behaviour):
            proc = subprocess.run(
                [os.environ[binary_env_var], behaviour],
                capture_output=True,
                encoding="utf8",
            )
            self.assertEqual(proc.returncode, status, proc.stderr)
            self.assertIn(stderr, proc.stderr)

    def check(self, behaviour: str, status: int, stderr: str = "") -> None:
        for binary in ("EXIT_STATUS_BIN", "ASYNC_EXIT_STATUS_BIN"):
            self.assert_exits(binary, behaviour, status, stderr)

    def test_none_is_success(self) -> None:
        self.check("none", 0)

    def test_zero_is_success(self) -> None:
        self.check("zero", 0)

    def test_int_is_the_exit_code(self) -> None:
        self.check("three", 3)

    def test_string_is_an_error_message(self) -> None:
        self.check("message", 1, stderr="it broke")

    def test_explicit_sys_exit_still_wins(self) -> None:
        self.check("sys-exit", 5)


if __name__ == "__main__":
    unittest.main()
