# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import asyncio
import os
import signal
from pathlib import Path

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase, dump_buck_state


class DumpBuckStateTest(BuckTestCase):
    @buck_test()
    async def test_dump_shows_the_running_build(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        """
        The dump a timed-out e2e test writes names the command its daemon is running and the
        actions that command is waiting on, and includes that command's event log.
        """
        pid_path = tmp_path / "pids"
        pid_path.mkdir()
        opts = ["-c", f"test.pids={pid_path}", "-c", "test.duration=60"]
        await buck.audit("providers", ":slow", *opts)
        build = await buck.build(*opts, "--local-only", ":slow").start()
        for _i in range(30):
            await asyncio.sleep(1)
            if os.listdir(pid_path):
                break
        else:
            raise Exception("The actions never started")

        dest = tmp_path / "dump"
        dump_buck_state(buck, dest)

        status = (dest / "buck2_status.txt").read_text()
        # `log what-up` writes to stderr.
        what_up = (dest / "buck2_what_up.stderr.txt").read_text()
        self.assertIn('"build"', status)
        self.assertIn("root//:slow", what_up)
        event_logs = [
            p.name for p in dest.iterdir() if p.name.endswith("_events.pb.zst")
        ]
        self.assertEqual(len(event_logs), 1, f"files: {os.listdir(dest)}")
        self.assertIn("_build_", event_logs[0])

        build.send_signal(signal.SIGINT)
        await build.communicate()

    @buck_test()
    async def test_dump_out_of_time_writes_errors_and_returns(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        dest = tmp_path / "dump"
        dump_buck_state(buck, dest, seconds=0)
        self.assertEqual(
            sorted(p.name for p in dest.iterdir()),
            [
                "buck2_event_log.error.txt",
                "buck2_status.error.txt",
                "buck2_what_up.error.txt",
            ],
        )
