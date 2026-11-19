#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import json
import pathlib
import runpy
import shutil
import sys
import tempfile
import unittest
from unittest import mock

# Stands in for rustc: writes the diagnostics it is told to, writes its output or not, and exits
# with the code it is told to. Driven by the environment `rustc_action.py` gives it.
FAKE_RUSTC = (
    "import os, sys; "
    "[sys.stderr.write(d + chr(10)) for d in os.environ.get('FAKE_DIAGS', '').split('|') if d]; "
    "os.environ.get('FAKE_WRITE') == '1' and open(os.environ['FAKE_OUT'], 'w').write('fresh-rmeta'); "
    "sys.exit(int(os.environ.get('FAKE_EXIT', '0')))"
)
ERROR_DIAG = json.dumps(
    {
        "level": "error",
        "message": "mismatched types",
        "rendered": "error[E0308]: mismatched types",
    }
)
DENY_UNUSED_DIAG = json.dumps({"lint_level": "deny", "unused_extern_names": ["serde"]})
SHORT = "LPPMDI/libfoo.rmeta"


def run_tool(module: str, argv: list) -> int:
    """Run one of the tools as its `python_bootstrap_binary` would, in-process."""
    with mock.patch.object(sys, "argv", [module] + argv):
        try:
            runpy.run_module(module, run_name="__main__")
        except SystemExit as e:
            return e.code or 0
    raise AssertionError(f"{module} did not exit")


class FailureFilterTest(unittest.TestCase):
    """`rust_compile(infallible_diagnostics = True)` runs `rustc_action` with `--failure-filter`
    and the `[check]` subtarget is `failure_filter_action` over its build status."""

    def setUp(self) -> None:
        self.tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.out = self.tmp / "LPPMDI" / "libfoo.rmeta"
        self.out.parent.mkdir()
        self.status = self.tmp / "build_status.json"

    def compile(
        self,
        *,
        diags: list,
        write: bool,
        exit_code: int,
        keep_outputs_on_error: bool = False,
    ) -> int:
        """One `rustc_action` run. The output path is kept between runs, as it is for an
        incremental compile whose action does not clean its outputs."""
        args = [
            "--env=FAKE_DIAGS=" + "|".join(diags),
            f"--env=FAKE_OUT={self.out}",
            "--env=FAKE_WRITE=" + ("1" if write else "0"),
            f"--env=FAKE_EXIT={exit_code}",
            f"--diag-json={self.tmp / 'diag.json'}",
            f"--diag-txt={self.tmp / 'diag.txt'}",
            "--buck-target=//foo:foo",
            f"--failure-filter={self.status}",
            "--required-output",
            SHORT,
            str(self.out),
            *(["--keep-outputs-on-error"] if keep_outputs_on_error else []),
            sys.executable,
            "-c",
            FAKE_RUSTC,
            "--rustc-action-separator",
        ]
        argfile = self.tmp / "rustc.args"
        argfile.write_text("".join(arg + "\n" for arg in args))
        return run_tool("rustc_action", [f"@{argfile}"])

    def check(self) -> tuple:
        """The `[check]` action: returns its exit code and the copied rmeta, if any."""
        final = self.tmp / "check" / "libfoo.rmeta"
        shutil.rmtree(final.parent, ignore_errors=True)
        final.parent.mkdir()
        code = run_tool(
            "failure_filter_action",
            [
                "--stderr",
                str(self.tmp / "diag.txt"),
                "--required-file",
                SHORT,
                str(self.out),
                str(final),
                "--build-status",
                str(self.status),
            ],
        )
        return code, (final.read_text() if final.exists() else None)

    def build_status(self) -> dict:
        return json.loads(self.status.read_text())

    def test_check_fails_after_a_failed_compile_despite_a_previous_rmeta(self) -> None:
        self.assertEqual(self.compile(diags=[], write=True, exit_code=0), 0)
        self.assertEqual(self.check(), (0, "fresh-rmeta"))

        # The source now has a type error: rustc fails and writes nothing.
        self.assertEqual(self.compile(diags=[ERROR_DIAG], write=False, exit_code=1), 0)
        self.assertEqual(self.build_status(), {"status": 1, "files": []})
        self.assertEqual(self.check(), (1, None))

    def test_check_fails_on_a_placeholder_from_a_failed_compile(self) -> None:
        self.assertEqual(self.compile(diags=[ERROR_DIAG], write=False, exit_code=1), 0)
        self.assertEqual(self.check(), (1, None))

        self.assertEqual(self.compile(diags=[ERROR_DIAG], write=False, exit_code=1), 0)
        self.assertEqual(self.check(), (1, None))

    def test_check_fails_on_a_silent_unused_dependency_error(self) -> None:
        self.assertEqual(
            self.compile(diags=[DENY_UNUSED_DIAG], write=True, exit_code=0), 0
        )
        self.assertEqual(self.build_status(), {"status": 1, "files": []})
        self.assertEqual(self.check(), (1, None))

    def test_check_succeeds_after_a_successful_compile(self) -> None:
        self.assertEqual(self.compile(diags=[], write=True, exit_code=0), 0)
        self.assertEqual(self.build_status(), {"status": 0, "files": [SHORT]})
        self.assertEqual(self.check(), (0, "fresh-rmeta"))

    def test_expand_succeeds_on_the_expansion_of_a_crate_with_errors(self) -> None:
        # `-Zunpretty=expanded` writes the expansion, then reports the expanded code's errors.
        self.assertEqual(
            self.compile(
                diags=[ERROR_DIAG], write=True, exit_code=1, keep_outputs_on_error=True
            ),
            0,
        )
        self.assertEqual(self.build_status(), {"status": 1, "files": [SHORT]})
        self.assertEqual(self.check(), (0, "fresh-rmeta"))

    def test_expand_fails_on_a_previous_expansion_after_a_failed_compile(self) -> None:
        self.assertEqual(
            self.compile(diags=[], write=True, exit_code=0, keep_outputs_on_error=True),
            0,
        )
        self.assertEqual(self.check(), (0, "fresh-rmeta"))

        # The source now fails to parse: rustc fails and writes nothing.
        self.assertEqual(
            self.compile(
                diags=[ERROR_DIAG], write=False, exit_code=1, keep_outputs_on_error=True
            ),
            0,
        )
        self.assertEqual(self.build_status(), {"status": 1, "files": []})
        self.assertEqual(self.check(), (1, None))
