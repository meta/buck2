# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import json
import typing

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


async def _snapshot_events(buck: Buck) -> list[dict[str, typing.Any]]:
    """Returns the Snapshot payloads from the most recent invocation's log."""
    out = await buck.log("show")
    snapshots: list[dict[str, typing.Any]] = []
    for line in out.stdout.splitlines():
        if not line:
            continue
        event = json.loads(line)
        instant = (
            event.get("Event", {}).get("data", {}).get("Instant", {}).get("data", {})
        )
        if "Snapshot" in instant:
            snapshots.append(instant["Snapshot"])
    return snapshots


def append_tokio_workers_config(buck: Buck) -> None:
    (buck.cwd / ".bucksettings.toml").write_text("[daemon]\nnum_tokio_workers = 42\n")


async def start_daemon_and_get_status(
    buck: Buck, include_tokio_runtime_metrics: bool = False
) -> dict[str, typing.Any]:
    # Start the daemon
    await buck.targets(":")

    status_result = await (
        buck.status("--include-tokio-runtime-metrics")
        if include_tokio_runtime_metrics
        else buck.status()
    )
    status_data = json.loads(status_result.stdout)
    return status_data


class DaemonTokioMetricsTest(BuckTestCase):
    @buck_test(skip_for_os=["darwin", "windows"])
    async def test_default_output(self, buck: Buck) -> None:
        status = await start_daemon_and_get_status(buck)
        assert "tokio_runtime_metrics" not in status

    @buck_test(skip_for_os=["darwin", "windows"])
    async def test_includes_tokio(self, buck: Buck) -> None:
        status = await start_daemon_and_get_status(
            buck, include_tokio_runtime_metrics=True
        )
        assert "tokio_runtime_metrics" in status
        metrics = status["tokio_runtime_metrics"]
        assert "global_queue_depth" in metrics
        assert "num_alive_tasks" in metrics
        assert "num_workers" in metrics

    @buck_test(skip_for_os=["darwin", "windows"])
    async def test_notices_tokio_worker_override(self, buck: Buck) -> None:
        append_tokio_workers_config(buck)
        status = await start_daemon_and_get_status(
            buck, include_tokio_runtime_metrics=True
        )
        assert "tokio_runtime_metrics" in status
        metrics = status["tokio_runtime_metrics"]
        assert "num_workers" in metrics
        assert metrics["num_workers"] == 42  # from modification below

    @buck_test()
    async def test_snapshot_events_carry_tokio_runtime_stats(self, buck: Buck) -> None:
        # Snapshots should always include tokio_runtime_stats — there's no toggle.
        await buck.targets(":")
        snapshots = await _snapshot_events(buck)
        assert len(snapshots) > 0, "expected at least one snapshot event in the log"
        populated = [s for s in snapshots if s.get("tokio_runtime_stats")]
        assert len(populated) > 0, (
            "expected at least one snapshot to carry tokio_runtime_stats"
        )
        stats = populated[-1]["tokio_runtime_stats"]
        assert "workers" in stats
        assert len(stats["workers"]) > 0
        # At least one worker should have done some work, exposing cumulative
        # counters. (Default-zero u64 fields are omitted by proto3 JSON, so we
        # look for a worker that has any of them present.)
        counter_fields = ("park_unpark_count", "poll_count", "total_busy_duration_us")
        assert any(any(f in w for f in counter_fields) for w in stats["workers"]), (
            f"expected some worker to expose any of {counter_fields}, got {stats['workers']}"
        )
