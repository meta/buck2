# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import asyncio
import json
import re
import signal
from pathlib import Path
from typing import List

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase

ON = ["--target-platforms", "root//:base", "-c", "buck2.dedupe_in_flight_actions=true"]
OFF = ["--target-platforms", "root//:base", "-c", "buck2.dedupe_in_flight_actions=false"]


def _use_runs_log(buck: Buck, tmp_path: Path) -> Path:
    """Points the fixture's actions at a log they append their target names to."""
    runs = tmp_path / "runs.log"
    runs.write_text("")
    (buck.cwd / "runs.bzl").write_text(f'RUNS = "{runs}"\n')
    return runs


def _runs(runs: Path, name: str) -> int:
    return runs.read_text().splitlines().count(name)


async def _executors(buck: Buck, name: str, *log: str) -> List[str]:
    """How each configuration of `name` ran, from `buck2 log what-ran`."""
    result = await buck.log("what-ran", "--format", "json", *log)
    return [
        entry["reproducer"]["executor"]
        for entry in map(json.loads, result.stdout.splitlines())
        if f":{name} " in entry["identity"]
    ]


async def _in_flight_waits(buck: Buck) -> int:
    """How many times an action of the last command waited for an identical one in flight."""
    events = (await buck.log("show")).stdout
    return sum(
        1
        for line in events.splitlines()
        if json.loads(line)
        .get("Event", {})
        .get("data", {})
        .get("SpanStart", {})
        .get("data", {})
        .get("ExecutorStage", {})
        .get("stage", {})
        .get("InFlightWait")
        is not None
    )


def _failed_configurations(stderr: str, name: str) -> int:
    return len(set(re.findall(rf"Action failed: root//:{name} \([^)]*\)", stderr)))


async def _outputs(buck: Buck, *args: str) -> str:
    result = await buck.build("root//:all", *args)
    out = result.get_build_report().output_for_target("root//:all")
    return "".join(f"{f.name}: {f.read_text()}" for f in sorted(out.iterdir()))


class DedupeInFlightActionsTest(BuckTestCase):
    @buck_test(skip_for_os=["windows"])
    async def test_key_off_every_configuration_executes(self, buck: Buck, tmp_path: Path) -> None:
        runs = _use_runs_log(buck, tmp_path)
        await buck.build("root//:all", *OFF)
        assert _runs(runs, "common") == 4
        assert _runs(runs, "per_variant") == 4

    @buck_test(skip_for_os=["windows"])
    async def test_key_on_each_digest_executes_once(self, buck: Buck, tmp_path: Path) -> None:
        runs = _use_runs_log(buck, tmp_path)
        result = await buck.build("root//:all", *ON)
        # `common` reads no axis, `per_variant` one of two.
        assert _runs(runs, "common") == 1
        assert _runs(runs, "per_variant") == 2

        executors = await _executors(buck, "common")
        assert sorted(executors) == ["InFlight"] * 3 + ["Local"], executors

        # Every follower waits in an in_flight_wait stage, and the summary counts it.
        followers = (await buck.log("what-ran", "--format", "json")).stdout.count(
            '"executor":"InFlight"'
        )
        waits = await _in_flight_waits(buck)
        assert followers > 0 and waits == followers, (followers, waits)
        assert f"Deduplicated by buck2: {followers}" in result.stderr, result.stderr

    @buck_test(skip_for_os=["windows"])
    async def test_outputs_match_with_key_off_and_on(self, buck: Buck, tmp_path: Path) -> None:
        _use_runs_log(buck, tmp_path)
        off = await _outputs(buck, *OFF)
        await buck.clean()
        on = await _outputs(buck, *ON)
        assert off and off == on, (off, on)

    @buck_test(skip_for_os=["windows"])
    async def test_leader_failure_is_shared(self, buck: Buck, tmp_path: Path) -> None:
        runs = _use_runs_log(buck, tmp_path)
        failure = await expect_failure(buck.build("root//:all_broken", "--keep-going", *ON))
        assert _runs(runs, "broken") == 1
        assert _failed_configurations(failure.stderr, "broken") == 4, failure.stderr

    @buck_test(skip_for_os=["windows"])
    async def test_leader_killed_by_a_signal_is_not_shared(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        runs = _use_runs_log(buck, tmp_path)
        failure = await expect_failure(buck.build("root//:all_killed", "--keep-going", *ON))
        # Each follower runs itself.
        assert _runs(runs, "killed") == 4
        assert _failed_configurations(failure.stderr, "killed") == 4, failure.stderr
        # The three followers waited for the leader once each; none waited again for another
        # follower to lead.
        assert await _in_flight_waits(buck) == 3

    @buck_test(skip_for_os=["windows"])
    async def test_leader_that_times_out_is_not_shared(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        runs = _use_runs_log(buck, tmp_path)
        failure = await expect_failure(buck.build("root//:all_times_out", "--keep-going", *ON))
        # Each follower runs itself, and times out itself.
        assert _runs(runs, "times_out") == 4
        assert _failed_configurations(failure.stderr, "times_out") == 4, failure.stderr
        # The three followers waited for the leader once each; none waited again for another
        # follower to lead.
        assert await _in_flight_waits(buck) == 3

    @buck_test(skip_for_os=["windows"])
    async def test_error_handlers_read_the_failed_leaders_outputs(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        runs = _use_runs_log(buck, tmp_path)
        failure = await expect_failure(buck.build("root//:all_diagnosed", "--keep-going", *ON))
        assert _runs(runs, "diagnosed") == 1
        assert _failed_configurations(failure.stderr, "diagnosed") == 4, failure.stderr
        assert failure.stderr.count("diag_E42") >= 4, failure.stderr
        assert "diag_missing" not in failure.stderr, failure.stderr

    @buck_test(skip_for_os=["windows"])
    async def test_two_commands_share_a_leader(self, buck: Buck, tmp_path: Path) -> None:
        runs = _use_runs_log(buck, tmp_path)
        x86_log, arm_log = tmp_path / "x86.pb.zst", tmp_path / "arm.pb.zst"
        # One daemon up front, so neither command spends `common`'s run starting it.
        await buck.server()
        await asyncio.gather(
            buck.build("root//:common_x86", "--event-log", str(x86_log), *ON),
            buck.build("root//:common_arm", "--event-log", str(arm_log), *ON),
        )
        # Each command builds two configurations: one ran `common`, and the other three took it
        # in flight, so the second command's two did not hit the cache after the first finished.
        assert _runs(runs, "common") == 1
        executors = await _executors(buck, "common", str(x86_log)) + await _executors(
            buck, "common", str(arm_log)
        )
        assert executors.count("InFlight") == 3, executors

    @buck_test(skip_for_os=["windows"])
    async def test_cancelling_the_leaders_command_hands_its_work_on(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        runs = _use_runs_log(buck, tmp_path)
        leader = await buck.build("root//:slow_x86", *ON).start()
        for _ in range(300):
            if _runs(runs, "slow"):
                break
            await asyncio.sleep(0.1)
        else:
            raise AssertionError("the leader never started")
        follower_log = tmp_path / "follower.pb.zst"
        follower = await buck.build(
            "root//:slow_arm", "--event-log", str(follower_log), *ON
        ).start()
        await asyncio.sleep(2)
        leader.send_signal(signal.SIGINT)
        assert await leader.wait() != 0
        assert await asyncio.wait_for(follower.wait(), timeout=60) == 0
        # The cancelled leader and its successor.
        assert _runs(runs, "slow") == 2
        # The follower waited on the leader before it took over; otherwise nothing was handed on.
        events = (await buck.log("show", str(follower_log))).stdout
        assert '"InFlightWait"' in events, "the follower never waited on the leader"

    @buck_test(skip_for_os=["windows"])
    async def test_dep_file_hit_is_shared(self, buck: Buck, tmp_path: Path) -> None:
        runs = _use_runs_log(buck, tmp_path)
        await buck.build("root//:all_depf", *ON)
        targets = buck.cwd / "TARGETS.fixture"
        targets.write_text(targets.read_text().replace("unused v1", "unused v2"))
        runs.write_text("")
        result = await buck.build("root//:all_depf", *ON)

        assert _runs(runs, "depf") == 0
        assert _runs(runs, "unused") == 1
        out = result.get_build_report().output_for_target("root//:all_depf")
        assert all(f.read_text() == "used\n" for f in out.iterdir())
        # The first configuration to arrive hits the dep file cache; the others take its result
        # in flight. At least one must, or the hit was not shared.
        executors = await _executors(buck, "depf")
        assert executors.count("LocalDepFileCache") >= 1, executors
        assert executors.count("InFlight") >= 1, executors
        assert len(executors) == 4, executors

    @buck_test(skip_for_os=["windows"])
    async def test_commands_with_different_options_do_not_share(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        # A command's executor options can change what an action produces, so a follower only
        # takes a leader's result when its command runs with the same ones: here one command
        # reads the remote cache and the other doesn't.
        runs = _use_runs_log(buck, tmp_path)
        x86_log, arm_log = tmp_path / "x86.pb.zst", tmp_path / "arm.pb.zst"
        await buck.server()
        await asyncio.gather(
            buck.build("root//:slow_x86", "--event-log", str(x86_log), *ON),
            buck.build(
                "root//:slow_arm", "--no-remote-cache", "--event-log", str(arm_log), *ON
            ),
        )
        # One leader per command, each followed by its command's other configuration.
        assert _runs(runs, "slow") == 2
        for log in [x86_log, arm_log]:
            executors = await _executors(buck, "slow", str(log))
            assert sorted(executors) == ["InFlight", "Local"], (log.name, executors)

    @buck_test(skip_for_os=["windows"])
    async def test_cancelling_a_followers_command_leaves_the_leader(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        runs = _use_runs_log(buck, tmp_path)
        leader = await buck.build("root//:slow_x86", *ON).start()
        for _ in range(300):
            if _runs(runs, "slow"):
                break
            await asyncio.sleep(0.1)
        else:
            raise AssertionError("the leader never started")
        follower_log = tmp_path / "follower.pb.zst"
        follower = await buck.build(
            "root//:slow_arm", "--event-log", str(follower_log), *ON
        ).start()
        await asyncio.sleep(2)
        follower.send_signal(signal.SIGINT)
        assert await follower.wait() != 0
        assert await asyncio.wait_for(leader.wait(), timeout=60) == 0
        # The leader ran once, for every configuration, and the cancelled followers ran nothing.
        assert _runs(runs, "slow") == 1
        events = (await buck.log("show", str(follower_log))).stdout
        assert '"InFlightWait"' in events, "the cancelled command never followed the leader"

    @buck_test(skip_for_os=["windows"])
    async def test_followers_record_their_own_dep_files(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        runs = _use_runs_log(buck, tmp_path)
        await buck.build("root//:all_depf", *ON)
        assert _runs(runs, "depf") == 1
        executors = await _executors(buck, "depf")
        assert sorted(executors) == ["InFlight"] * 3 + ["Local"], executors
        targets = buck.cwd / "TARGETS.fixture"
        targets.write_text(targets.read_text().replace("unused v1", "unused v2"))
        runs.write_text("")
        # With the key off each configuration looks up only its own dep file: another
        # configuration's entry matches only identical inputs, and an input changed.
        await buck.build("root//:all_depf", *OFF)
        assert _runs(runs, "depf") == 0
        executors = await _executors(buck, "depf")
        assert executors == ["LocalDepFileCache"] * 4, executors

    @buck_test(skip_for_os=["windows"])
    async def test_followers_fill_their_own_offline_cache(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        runs = _use_runs_log(buck, tmp_path)
        await buck.debug("trace-io", "enable")
        await buck.build("root//:all_offline", *ON)
        assert _runs(runs, "offline") == 1
        # The offline cache has a path per configuration, and each gets a copy.
        listed = (await buck.audit("deferred-materializer", "list")).stdout
        copies = [
            path
            for path, _ in (line.split("\t", 1) for line in listed.splitlines() if "\t" in line)
            if "/offline-cache/" in path and "__offline__" in path
        ]
        assert len(copies) == 4, copies
