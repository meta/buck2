#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Fast wrapper-protocol tests for rustc_action.py.

Exercises the wrapper as a subprocess with a fake driver that records argv,
selected environment, and the compiler response-file contents. Run directly
(`python3 rustc_action_test.py`) or via `buck2 test`.
"""

import json
import os
import subprocess
import sys
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "rustc_action.py")

FAKE_DRIVER = """\
import json
import os
import sys

record = {
    "argv": sys.argv[1:],
    "env": {
        key: os.environ.get(key)
        for key in ("HEAD", "FOO", "FAKE_WANT")
    },
    "compiler_file": None,
}
for token in sys.argv[1:]:
    if token.startswith("@"):
        with open(token[1:], encoding="utf-8") as handle:
            record["compiler_file"] = handle.read()
        break
if os.environ.get("FAKE_JSON_STDERR") == "1":
    sys.stderr.write('{"rendered":"boom","level":"error"}\\n')
    sys.stderr.flush()
with open(os.environ["FAKE_RECORD"], "w", encoding="utf-8") as handle:
    json.dump(record, handle)
sys.exit(int(os.environ.get("FAKE_EXIT", "0")))
"""


class WrapperTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.driver = os.path.join(self.tmp.name, "fake_driver.py")
        with open(self.driver, "w", encoding="utf-8") as handle:
            handle.write(FAKE_DRIVER)
        self.record_path = os.path.join(self.tmp.name, "record.json")

    def run_wrapper(self, argv, env_extra=None, prefix=()):
        """Run the wrapper with driver argv; return (exit, record, stderr).

        `prefix` elements go between the driver and
        `--rustc-action-separator` (the executable command prefix, kept
        raw); `argv` elements go after it (compiler arguments, expanded).
        """
        control_env = {
            "FAKE_RECORD": self.record_path,
            "FAKE_EXIT": "0",
        }
        if env_extra:
            control_env.update(env_extra)
        wrapper_env = [f"--env={key}={value}" for key, value in control_env.items()]
        cmd = (
            [sys.executable, SCRIPT]
            + wrapper_env
            + ["--passthrough-stderr", sys.executable, self.driver]
            + list(prefix)
            + ["--rustc-action-separator"]
            + argv
        )
        proc = subprocess.run(cmd, capture_output=True, text=True, cwd=self.tmp.name)
        with open(self.record_path, encoding="utf-8") as handle:
            record = json.load(handle)
        return proc.returncode, record, proc.stderr

    def test_no_boundary_keeps_response_file_layout(self):
        code, record, _ = self.run_wrapper(
            ["--crate-name", "example", "--env-set=HEAD=x"]
        )
        self.assertEqual(code, 0)
        # Exactly one response file, then nothing.
        self.assertEqual(len(record["argv"]), 1)
        self.assertTrue(record["argv"][0].startswith("@"))
        self.assertNotIn("--", record["argv"])
        # Preprocessing still applies before the (absent) boundary.
        self.assertIn("--crate-name", record["compiler_file"])
        self.assertIn("example", record["compiler_file"])
        self.assertNotIn("--env-set", record["compiler_file"])
        self.assertEqual(record["env"]["HEAD"], "x")

    def test_empty_tail_emits_separator(self):
        code, record, _ = self.run_wrapper(
            ["--crate-name", "example", "--program-args"]
        )
        self.assertEqual(code, 0)
        self.assertEqual(record["argv"][1:], ["--"])

    def test_tail_verbatim_and_head_transformed(self):
        code, record, _ = self.run_wrapper(
            [
                "--env-set=HEAD=x",
                "--program-args",
                "--env-set=FOO=bar",
                "plain",
                "--remap-path-prefix=/old=./new",
            ]
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            record["argv"][1:],
            ["--", "--env-set=FOO=bar", "plain", "--remap-path-prefix=/old=./new"],
        )
        self.assertEqual(record["env"]["HEAD"], "x")
        self.assertIsNone(record["env"]["FOO"])
        self.assertNotIn("--env-set", record["compiler_file"])

    def test_later_boundaries_stay_literal(self):
        code, record, _ = self.run_wrapper(
            ["--program-args", "one", "--", "two", "--program-args", "three"]
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            record["argv"][1:], ["--", "one", "--", "two", "--program-args", "three"]
        )

    def test_at_file_looking_tail_never_expands(self):
        real = os.path.join(self.tmp.name, "payload.txt")
        with open(real, "w", encoding="utf-8") as handle:
            handle.write("SHOULD NOT EXPAND")
        code, record, _ = self.run_wrapper(
            ["--program-args", "@/nonexistent-xyz", "@" + real]
        )
        self.assertEqual(code, 0)
        self.assertEqual(record["argv"][1:], ["--", "@/nonexistent-xyz", "@" + real])
        # The recorded file is the wrapper's compiler file, not the tail.
        self.assertNotIn("SHOULD NOT EXPAND", record["compiler_file"])

    def test_macro_looking_values(self):
        code, record, _ = self.run_wrapper(
            [
                "--extern=$(abspath %s)" % self.tmp.name,
                "--program-args",
                "$(cat /nonexistent-xyz)",
                "$(abspath /tmp)",
            ]
        )
        self.assertEqual(code, 0)
        # Trusted expansion before the boundary only.
        self.assertIn(
            "--extern=%s" % os.path.abspath(self.tmp.name), record["compiler_file"]
        )
        self.assertNotIn("$(", record["compiler_file"])
        self.assertEqual(
            record["argv"][1:],
            ["--", "$(cat /nonexistent-xyz)", "$(abspath /tmp)"],
        )

    def test_prefix_flags_raw_and_ordered(self):
        # Prefix flags stay raw (a literal `$(...)` must not expand);
        # compiler arguments after the separator still expand.
        code, record, _ = self.run_wrapper(
            ["--extern=$(abspath %s)" % self.tmp.name, "--program-args", "prog"],
            prefix=["-Zmiri-first", "-Zmiri-env-set=FAKE_WANT=lit$(cat x)"],
        )
        self.assertEqual(code, 0)
        argv = record["argv"]
        self.assertEqual(argv[0], "-Zmiri-first")
        self.assertEqual(argv[1], "-Zmiri-env-set=FAKE_WANT=lit$(cat x)")
        self.assertTrue(argv[2].startswith("@"))
        self.assertEqual(argv[3:], ["--", "prog"])
        self.assertNotIn("-Zmiri", record["compiler_file"])
        self.assertIn(
            "--extern=%s" % os.path.abspath(self.tmp.name), record["compiler_file"]
        )

    def test_passthrough_diagnostics_and_exit(self):
        code, record, stderr = self.run_wrapper(
            ["--program-args"],
            env_extra={"FAKE_JSON_STDERR": "1", "FAKE_EXIT": "3"},
        )
        self.assertEqual(code, 3)
        self.assertIn('{"rendered":"boom","level":"error"}', stderr)

    def test_echo_shows_boundary_and_raw_tail(self):
        out_path = os.path.join(self.tmp.name, "echo.txt")
        cmd = [
            sys.executable,
            SCRIPT,
            "--echo",
            out_path,
            self.driver,
            "--rustc-action-separator",
            "--crate-name",
            "example",
            "--program-args",
            "--env-set=FOO=bar",
        ]
        proc = subprocess.run(cmd, capture_output=True, text=True)
        self.assertEqual(proc.returncode, 0)
        with open(out_path, encoding="utf-8") as handle:
            echoed = handle.read().splitlines()
        self.assertIn("--program-args", echoed)
        self.assertIn("--env-set=FOO=bar", echoed)


if __name__ == "__main__":
    unittest.main()
