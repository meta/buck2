# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import contextlib
import io
import json
import os
import sys
import tempfile
import unittest
from unittest import mock

import worker_tool_runner


class ScriptedWorker:
    """Stands in for the worker process: it echoes the handshake, answers each command with
    the next scripted exit code (or a worker error for `TYPE_ERROR`) after writing that
    command's stderr file, and acknowledges the termination."""

    def __init__(self, exit_codes):
        self._exit_codes = list(exit_codes)
        self._pending = b""
        self.args = ["scripted-worker"]
        self.stdin = self
        self.stdout = io.BytesIO()
        self.stderr = io.BytesIO()

    def __enter__(self):
        return self

    def __exit__(self, *exc_info):
        return False

    def write(self, data):
        self._pending += data

    def flush(self):
        text = self._pending.decode("utf-8")
        self._pending = b""
        if text == worker_tool_runner.EXIT_DATA:
            self._reply(text)
        elif text.startswith(worker_tool_runner.START_HANDSHAKE_PREFIX):
            self._reply(text)
        else:
            command = json.loads(text[1:])
            with open(command["stderr_path"], "w") as stderr_file:
                stderr_file.write("command {} stderr\n".format(command["id"]))
            outcome = self._exit_codes[command["id"] - 1]
            if outcome == worker_tool_runner.TYPE_ERROR:
                result = {
                    "id": command["id"],
                    "type": worker_tool_runner.TYPE_ERROR,
                    "message": "scripted worker error",
                }
            else:
                result = {
                    "id": command["id"],
                    "type": worker_tool_runner.TYPE_RESULT,
                    "exit_code": outcome,
                }
            self._reply(worker_tool_runner.START_MESSAGE_PREFIX + json.dumps(result))

    def _reply(self, text):
        read_position = self.stdout.tell()
        self.stdout.seek(0, io.SEEK_END)
        self.stdout.write(text.encode("utf-8"))
        self.stdout.seek(read_position)


def run_batch(exit_codes):
    """Runs one aggregate command args file through `main` against a scripted worker and
    returns the runner's exit code and what it printed."""
    with tempfile.TemporaryDirectory() as tmp_dir:
        batch_path = os.path.join(tmp_dir, "batch.json")
        with open(batch_path, "w") as batch_file:
            json.dump(
                {
                    "version": worker_tool_runner.AGGREGATE_COMMAND_ARGS_VERSION,
                    "commands": [{"command": "transform"}] * len(exit_codes),
                },
                batch_file,
            )
        argv = [
            "worker_tool_runner.py",
            "--worker-tool",
            "scripted-worker",
            "--command-args-file",
            batch_path,
            "--command-args-file-range",
            "0:{}".format(len(exit_codes)),
        ]
        printed = io.StringIO()
        with (
            mock.patch.object(sys, "argv", argv),
            mock.patch("subprocess.Popen", return_value=ScriptedWorker(exit_codes)),
            contextlib.redirect_stdout(printed),
        ):
            try:
                worker_tool_runner.main()
            except SystemExit as exit_request:
                return exit_request.code, printed.getvalue()
        raise AssertionError("main() returned without exiting")


class WorkerToolRunnerTest(unittest.TestCase):
    def test_a_failure_before_a_success_is_reported_with_its_output(self):
        exit_code, printed = run_batch([1, 0])
        self.assertEqual(exit_code, 1)
        self.assertIn("finished with exit code: 1", printed)
        self.assertIn("command 1 stderr", printed)
        self.assertNotIn("command 2 stderr", printed)

    def test_a_failure_as_the_last_command_is_reported(self):
        exit_code, printed = run_batch([0, 1])
        self.assertEqual(exit_code, 1)
        self.assertIn("command 2 stderr", printed)

    def test_a_worker_error_after_a_failure_keeps_the_failure(self):
        exit_code, printed = run_batch([2, worker_tool_runner.TYPE_ERROR])
        self.assertEqual(exit_code, 2)
        self.assertIn("finished with exit code: 2", printed)
        self.assertIn("command 1 stderr", printed)
        self.assertNotIn("command 2 stderr", printed)

    def test_a_worker_error_without_an_earlier_failure_is_reported(self):
        exit_code, printed = run_batch([0, worker_tool_runner.TYPE_ERROR])
        self.assertEqual(exit_code, 1)
        self.assertIn("command 2 stderr", printed)

    def test_an_all_successful_batch_exits_zero(self):
        exit_code, _ = run_batch([0, 0])
        self.assertEqual(exit_code, 0)

    def test_variables_are_expanded_by_whole_name_from_the_worker_env(self):
        envs = {"A": "x", "AB": "yy", "FROMFILE": "fileval"}
        args = ["$AB", "${AB}", "$FROMFILE", "$A/$AB", "${A}B", "$ABC", "$AX", "$"]
        with mock.patch.dict(os.environ, {"A": "x", "AB": "yy"}, clear=True):
            expanded = [
                worker_tool_runner._maybe_expand_worker_arg(arg, envs) for arg in args
            ]
        self.assertEqual(
            expanded, ["yy", "yy", "fileval", "x/yy", "xB", "$ABC", "$AX", "$"]
        )
