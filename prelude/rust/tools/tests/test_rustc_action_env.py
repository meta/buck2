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

# Stands in for rustc: records the environment it was started with.
FAKE_RUSTC = (
    "import json, os; "
    "json.dump(dict(os.environ), open(os.environ['FAKE_DUMP_ENV'], 'w'))"
)


def run_rustc_action(argfile: pathlib.Path) -> int:
    with mock.patch.object(sys, "argv", ["rustc_action", f"@{argfile}"]):
        try:
            runpy.run_module("rustc_action", run_name="__main__")
        except SystemExit as e:
            return e.code or 0
    raise AssertionError("rustc_action did not exit")


class EnvTransportTest(unittest.TestCase):
    """`rust_library(env = {"K": value})` reaches rustc through `process_env` in build.bzl, which
    escapes the value onto one line of the `rustc_action` argfile, and `rustc_action.py`, which
    decodes it into rustc's environment."""

    def setUp(self) -> None:
        self.tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)

    def transport(self, value: str) -> str:
        """The value rustc sees for `K`."""
        return self.rustc_env_for(escape_like_process_env(value))

    def rustc_env_for(self, argfile_value: str) -> str:
        """The value rustc sees for `K` when the argfile holds `argfile_value`."""
        dump = self.tmp / "env.json"
        dump.unlink(missing_ok=True)
        args = [
            f"--env=FAKE_DUMP_ENV={dump}",
            "--env=K=" + argfile_value,
            f"--diag-json={self.tmp / 'diag.json'}",
            f"--diag-txt={self.tmp / 'diag.txt'}",
            sys.executable,
            "-c",
            FAKE_RUSTC,
            "--rustc-action-separator",
        ]
        argfile = self.tmp / "rustc.args"
        # `at_argfile(allow_args = True)` writes one argument per line.
        argfile.write_text("".join(arg + "\n" for arg in args), encoding="utf-8")
        self.assertEqual(run_rustc_action(argfile), 0)
        return json.loads(dump.read_text())["K"]


def escape_like_process_env(value: str) -> str:
    """`_RUSTC_ACTION_ENV_ESCAPES` of `process_env` in build.bzl, applied in order."""
    for raw, escaped in [
        ("\\", "\\\\"),
        ("\n", "\\n"),
        ("\r", "\\r"),
        ("\x0b", "\\v"),
        ("\x0c", "\\f"),
        ("\x1c", "\\x1c"),
        ("\x1d", "\\x1d"),
        ("\x1e", "\\x1e"),
        ("\x85", "\\x85"),
        ("\u2028", "\\u2028"),
        ("\u2029", "\\u2029"),
    ]:
        value = value.replace(raw, escaped)
    return value


class EnvTransportFixedTest(EnvTransportTest):
    def test_values_reach_rustc_unchanged(self) -> None:
        for value in [
            "a\nb",
            "a\\nb",
            "\\n\n",
            "a\rb",
            "a\\\nb",
            "a\\\\b",
            "a\x0bb",
            "a\x0cb",
            "a\x1cb",
            "a\x1db",
            "a\x1eb",
            "a\x85b",
            "a\u2028b",
            "a\u2029b",
            "\\x41",
            "\\u0041",
            "trailing\\",
        ]:
            self.assertEqual(self.transport(value), value)

    def test_a_dollar_cat_in_a_value_is_still_expanded(self) -> None:
        secret = self.tmp / "secret.txt"
        secret.write_text("FILE-CONTENTS")
        self.assertEqual(self.transport(f"echo $(cat {secret})"), "echo FILE-CONTENTS")

    def test_a_backslash_that_starts_no_escape_is_kept(self) -> None:
        # `process_env` never writes these; an argfile written by hand may.
        for argfile_value in [
            "a\\b",
            "\\x4",
            "\\xZZ",
            "\\u00",
            "\\u00G1",
            "trailing\\",
        ]:
            self.assertEqual(self.rustc_env_for(argfile_value), argfile_value)
