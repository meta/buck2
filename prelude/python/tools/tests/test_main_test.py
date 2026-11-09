# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import faulthandler
import io
import os
import signal
import time
import unittest
from typing import Callable, IO

import buck_test_main


class _NoCoverage:
    def get_coverage(self) -> None:
        return None


def _subtests_case() -> tuple[type[unittest.TestCase], list[bool]]:
    after_skip: list[bool] = []

    class Subtests(unittest.TestCase):
        def test_subtest_fails(self) -> None:
            for i in range(2):
                with self.subTest(i=i):
                    self.assertEqual(i, 0, "value %d is not zero" % i)

        def test_subtest_skips(self) -> None:
            with self.subTest(case="unsupported"):
                self.skipTest("platform not supported")
            after_skip.append(True)

    return Subtests, after_skip


def _results_of(case: type[unittest.TestCase]) -> dict[str, dict[str, object]]:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(case)
    result = buck_test_main.BuckTestResult(
        io.StringIO(), False, 0, False, _NoCoverage(), suite
    )
    suite.run(result)
    return {str(entry["testCase"]): entry for entry in result.getResults()}


class SubtestReportingTest(unittest.TestCase):
    def test_a_skipped_subtest_leaves_the_test_passing(self) -> None:
        case, after_skip = _subtests_case()
        result = _results_of(case)["test_subtest_skips"]
        self.assertEqual(result["type"], "SUCCESS")
        self.assertEqual(result["message"], "Skipped: platform not supported")
        self.assertEqual(after_skip, [True])

    def test_a_failed_subtest_keeps_its_message(self) -> None:
        case, _ = _subtests_case()
        result = _results_of(case)["test_subtest_fails"]
        self.assertEqual(result["type"], "FAILURE")
        self.assertEqual(
            result["message"], "AssertionError: 1 != 0 : value 1 is not zero"
        )
        self.assertIn("test_subtest_fails", str(result["stacktrace"]))


def _waiting_for_sigterm() -> None:
    time.sleep(60)


def _sigterm_child(register: Callable[[IO[str]], None]) -> tuple[int, str]:
    """
    Forks a child that calls `register` with a file and then blocks in `_waiting_for_sigterm`,
    sends the child `SIGTERM`, and returns its wait status and what it wrote to the file.
    """
    out_r, out_w = os.pipe()
    ready_r, ready_w = os.pipe()
    pid = os.fork()
    if pid == 0:
        try:
            os.close(out_r)
            os.close(ready_r)
            register(os.fdopen(out_w, "w"))
            os.write(ready_w, b"x")
            _waiting_for_sigterm()
        finally:
            os._exit(1)
    os.close(out_w)
    os.close(ready_w)
    with os.fdopen(ready_r, "rb") as ready:
        ready.read(1)
    os.kill(pid, signal.SIGTERM)
    _, status = os.waitpid(pid, 0)
    with os.fdopen(out_r) as out:
        return status, out.read()


@unittest.skipUnless(
    hasattr(faulthandler, "register") and hasattr(os, "fork"),
    "needs faulthandler.register and fork",
)
class SigtermStacksTest(unittest.TestCase):
    def test_sigterm_prints_the_stack_then_kills(self) -> None:
        status, out = _sigterm_child(buck_test_main.print_stacks_on_sigterm)
        self.assertTrue(os.WIFSIGNALED(status), f"wait status {status}")
        self.assertEqual(os.WTERMSIG(status), signal.SIGTERM)
        self.assertIn("_waiting_for_sigterm", out)

    def test_without_it_sigterm_kills_silently(self) -> None:
        status, out = _sigterm_child(lambda file: None)
        self.assertTrue(os.WIFSIGNALED(status), f"wait status {status}")
        self.assertEqual(os.WTERMSIG(status), signal.SIGTERM)
        self.assertEqual(out, "")

    def test_a_forked_child_dies_of_sigterm_silently(self) -> None:
        # Like a `multiprocessing` worker that its pool stops with `SIGTERM`.
        out_r, out_w = os.pipe()
        pid = os.fork()
        if pid == 0:
            try:
                os.close(out_r)
                buck_test_main.print_stacks_on_sigterm(os.fdopen(out_w, "w"))
                worker = os.fork()
                if worker == 0:
                    os.kill(os.getpid(), signal.SIGTERM)
                    os._exit(1)
                _, status = os.waitpid(worker, 0)
                killed = (
                    os.WIFSIGNALED(status) and os.WTERMSIG(status) == signal.SIGTERM
                )
                os._exit(0 if killed else 2)
            finally:
                os._exit(1)
        os.close(out_w)
        _, status = os.waitpid(pid, 0)
        self.assertEqual(os.waitstatus_to_exitcode(status), 0)
        with os.fdopen(out_r) as out:
            self.assertEqual(out.read(), "")
