# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from pathlib import Path

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase, parametrize
from buck2.tests.e2e_util.helper.utils import filter_events, read_what_ran


async def unmaterialize_payload(buck: Buck, payload: Path) -> None:
    # Clean-stale should handle the local-only payload in three steps: upload
    # it to CAS, record it as "declared: cas download", then delete the local
    # copy. Currently the run treats its own upload finishing as outside
    # activity and interrupts itself after the second step, so the local copy
    # is never deleted.
    await buck.clean("--stale")
    results = await filter_events(
        buck, "Event", "data", "Instant", "data", "CleanStaleResult"
    )
    assert len(results) == 1, results
    assert results[0]["error"] is None, results
    stats = results[0]["stats"]
    # The delete counts as interrupted, the upload is not counted as succeeded
    # (that counter needs the delete to finish), and the payload stays on disk.
    assert stats["unmaterialization_upload_attempted_artifact_count"] == 1, stats
    assert stats["delete_interrupted_artifact_count"] == 1, stats
    assert stats["unmaterialization_upload_succeeded_artifact_count"] == 0, stats
    assert stats["unmaterialized_only_artifact_count"] == 0, stats
    assert payload.exists()
    audit = (await buck.audit("deferred-materializer", "list")).stdout
    assert "declared: cas download" in audit, audit


class CleanupUploadTest(BuckTestCase):
    @buck_test(skip_for_os=["darwin", "windows"])
    @parametrize("cleanup", [False, True])
    async def test_local_consumer_after_cleanup(
        self, buck: Buck, cleanup: bool
    ) -> None:
        before = await buck.build("root//:before", "--no-remote-cache")
        output = before.get_build_report().output_for_target("root//:before")
        assert output.read_text() == "payload"
        commands = await read_what_ran(buck)
        assert len(commands) == 2, commands
        assert all(
            command["reproducer"]["executor"] == "Local" for command in commands
        ), commands

        audit = (await buck.audit("deferred-materializer", "list")).stdout
        paths = [
            buck.cwd / line.split("\t", 1)[0]
            for line in audit.splitlines()
            if "\t" in line and line.split("\t", 1)[0].endswith("/payload.txt")
        ]
        assert len(paths) == 1, audit
        payload = paths[0]
        assert payload.read_text() == "payload"
        if cleanup:
            await unmaterialize_payload(buck, payload)

        # Without cleanup, the second build reads the payload already on disk.
        # With cleanup, the interrupted run left the payload on disk, but buck2
        # already tracks it as "download from CAS" instead of "on disk". So the
        # second build throws away the leftover local copy and downloads the
        # payload from CAS again (the `Materialization` check below). Either
        # way `write_payload` does not run again: the only command is the
        # `root//:after` consumer.
        after = await buck.build("root//:after", "--no-remote-cache")
        output = after.get_build_report().output_for_target("root//:after")
        assert output.read_text() == "payload"
        commands = await read_what_ran(buck)
        assert len(commands) == 1, commands
        assert "root//:after" in commands[0]["identity"], commands
        assert commands[0]["reproducer"]["executor"] == "Local", commands
        assert payload.read_text() == "payload"
        if cleanup:
            events = await filter_events(
                buck, "Event", "data", "SpanEnd", "data", "Materialization"
            )
            assert any(
                buck.cwd / event["path"] == payload and event["success"]
                for event in events
            ), events
