# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

"""
With `buck2.write_action_cache` on, a write action asks the action cache for its content under a
key that is a hash of its command line. A hit declares the output as a CAS download without
rendering the content, and nothing is written until something needs the file. A miss renders the
content and produces it the way a run action's output comes to be: written to its path when the
machine has room for local work, uploaded to the CAS and declared from there when it does not. A
write whose key falls within `buck2.write_action_cache_populate`, under an executor that publishes
local results (`allow_cache_uploads`), goes through the cache whatever the mode says, and a miss
first uploads the content and records the entry for later builds. Under `verify` every write is
rendered and declared as without the cache, and a hit is only compared against it.

The daemon is killed between recording and looking up, so that a hit says the key is the same
from one process to the next.
"""

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.api.buck_result import BuckResult
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase
from buck2.tests.e2e_util.helper.utils import (
    get_last_execution_kind,
    json_get,
    random_string,
)

ACTION_EXECUTION_KIND_SIMPLE = 4
ACTION_EXECUTION_KIND_DEFERRED = 6


def presence_check_rpcs(res: BuckResult) -> int:
    """How many CAS presence-check RPCs the daemon had issued by the end of the command."""
    return int(
        res.invocation_record()["last_snapshot"].get("re_presence_check_rpcs", 0)
    )


def counter(res: BuckResult, name: str) -> int:
    """A daemon counter at the end of the command, from its last snapshot."""
    return int(res.invocation_record()["last_snapshot"].get(name, 0))


async def soft_errors(buck: Buck, category: str) -> int:
    """How many soft errors of `category` the last command reported."""
    log = (await buck.log("show")).stdout.strip().splitlines()
    count = 0
    for line in log:
        e = json_get(line, "Event", "data", "Instant", "data", "StructuredError")
        if (
            e is not None
            and (e.get("soft_error_category") or {}).get("category") == category
        ):
            count += 1
    return count


# The keys of two fixed writes: `//:write` with the content `golden`, and `//:write_tset`'s
# projection of it. A change here is a change to what the key covers or how it is hashed, after
# which nothing recorded before is found: `KEY_VERSION` in `write_cache.rs` or `HASH_VERSION` in
# `builder.rs` is due a bump, and these an update with it. The tset key also covers the set's
# node identities, which include the configuration, so this repo's execution platform changing
# moves it too; the plain key does not move with that. The platform is the repo's own, with a
# fixed path separator, so the keys are the same on every host.
GOLDEN_KEYS = {
    "prelude//:write": "f2503b3ff7cb537d034d6d042b05b046ebe8980c4a8a2841a3bbc66c268ff2bf",
    "prelude//:write_tset": "6da1d4f6369bb634140fc022b3cf0f97534e2bd7b60a3bf844810ba921ad679f",
}


class WriteActionCacheTest(BuckTestCase):
    @buck_test(write_invocation_record=True)
    async def test_miss_is_written_when_there_is_room_for_it(self, buck: Buck) -> None:
        content = random_string()
        configs = [
            "-c",
            f"test.content={content}",
            "-c",
            "buck2.write_action_cache=use",
        ]

        # One write in an otherwise idle build is made locally, and nothing asks the CAS about
        # it: `write_cas_probe` is on in this repo, and the miss does not probe.
        res = await buck.build("//:write", *configs)
        output = res.get_build_report().output_for_target("//:write")
        assert await soft_errors(buck, "write_action_cache_lookup_failed") == 0
        assert counter(res, "write_action_cache_misses") >= 1
        assert counter(res, "write_action_cache_written") >= 1
        assert counter(res, "write_action_cache_uploaded") == 0
        assert presence_check_rpcs(res) == 0, "the miss did not probe the CAS"
        assert (
            await get_last_execution_kind(buck, category="write")
            == ACTION_EXECUTION_KIND_SIMPLE
        )
        assert output.read_text() == content

    @buck_test(write_invocation_record=True)
    async def test_miss_is_uploaded_when_there_is_no_room_for_it(
        self, buck: Buck
    ) -> None:
        content = random_string()
        configs = [
            "-c",
            f"test.content={content}",
            "-c",
            "buck2.write_action_cache=use",
        ]
        # The gate the hybrid executor runs local commands behind, with no room at all.
        env = {"BUCK2_TEST_LOW_PASS_FILTER_CAPACITY": "0"}

        # The miss is uploaded and declared as CAS content; neither memory nor disk holds the
        # bytes...
        res = await buck.build("//:write", "--materializations=none", *configs, env=env)
        output = res.get_build_report().output_for_target("//:write")
        assert counter(res, "write_action_cache_misses") >= 1
        assert counter(res, "write_action_cache_uploaded") >= 1
        assert counter(res, "write_action_cache_written") == 0
        assert (
            await get_last_execution_kind(buck, category="write")
            == ACTION_EXECUTION_KIND_DEFERRED
        )
        assert not output.exists()

        # ...until something needs the file, which then comes out of the CAS.
        await buck.build("//:write", *configs, env=env)
        assert output.read_text() == content

    # The populate failure where the cache cannot be written is a (quiet) soft error.
    @buck_test(write_invocation_record=True, allow_soft_errors=True)
    async def test_recorded_content_is_not_written_again(self, buck: Buck) -> None:
        content = random_string()
        configs = ["-c", f"test.content={content}"]

        res = await buck.build(
            "//:write", "-c", "buck2.write_action_cache_populate=1", *configs
        )
        if await soft_errors(buck, "write_action_cache_populate_failed"):
            # CI and devservers lack write access to the action cache (see `test_cache_uploads`),
            # and `pytest.skip` is a failure under tpx: where the entry cannot be recorded, what
            # there is to check is that the output still came to be, as a miss does.
            output = res.get_build_report().output_for_target("//:write")
            assert counter(res, "write_action_cache_written") >= 1
            assert (
                await get_last_execution_kind(buck, category="write")
                == ACTION_EXECUTION_KIND_SIMPLE
            )
            assert output.read_text() == content
            return

        # A second write of the same content, in another daemon, finds the entry: the output is
        # declared as a CAS download, and neither memory nor disk holds the bytes...
        await buck.kill()
        target = "//:write_again"
        res = await buck.build(
            target,
            "--materializations=none",
            "-c",
            "buck2.write_action_cache=use",
            *configs,
        )
        output = res.get_build_report().output_for_target(target)
        assert counter(res, "write_action_cache_hits") >= 1
        assert presence_check_rpcs(res) == 0, "the CAS was not probed"
        assert (
            await get_last_execution_kind(buck, category="write")
            == ACTION_EXECUTION_KIND_DEFERRED
        )
        assert not output.exists()

        # ...until something needs the file, which then comes out of the CAS.
        await buck.build(target, "-c", "buck2.write_action_cache=use", *configs)
        assert output.read_text() == content

    @buck_test(write_invocation_record=True, allow_soft_errors=True)
    async def test_different_content_is_a_miss(self, buck: Buck) -> None:
        recorded = random_string()
        await buck.build(
            "//:write",
            "-c",
            f"test.content={recorded}",
            "-c",
            "buck2.write_action_cache_populate=1",
        )
        await buck.kill()

        # The key is of the content's command line, so other content is a miss, written here.
        content = random_string()
        target = "//:write_again"
        res = await buck.build(
            target,
            "-c",
            f"test.content={content}",
            "-c",
            "buck2.write_action_cache=use",
        )
        output = res.get_build_report().output_for_target(target)
        assert counter(res, "write_action_cache_hits") == 0
        assert counter(res, "write_action_cache_misses") >= 1
        assert counter(res, "write_action_cache_written") >= 1
        assert (
            await get_last_execution_kind(buck, category="write")
            == ACTION_EXECUTION_KIND_SIMPLE
        )
        assert output.read_text() == content

    @buck_test(write_invocation_record=True, allow_soft_errors=True)
    async def test_verify_checks_a_hit_against_the_render(self, buck: Buck) -> None:
        content = random_string()
        configs = ["-c", f"test.content={content}"]

        await buck.build(
            "//:write", "-c", "buck2.write_action_cache_populate=1", *configs
        )
        recorded = await soft_errors(buck, "write_action_cache_populate_failed") == 0
        await buck.kill()

        # `verify` renders and declares the write as without the cache, and only compares.
        target = "//:write_again"
        res = await buck.build(
            target, "-c", "buck2.write_action_cache=verify", *configs
        )
        output = res.get_build_report().output_for_target(target)
        assert output.read_text() == content
        assert counter(res, "write_action_cache_mismatches") == 0
        assert await soft_errors(buck, "write_action_cache_mismatch") == 0
        if recorded:
            assert counter(res, "write_action_cache_verified") >= 1

    @buck_test(write_invocation_record=True, allow_soft_errors=True)
    async def test_transitive_set_projection_is_served(self, buck: Buck) -> None:
        content = random_string()
        configs = ["-c", f"test.content={content}"]
        expected = f"{content}\nchild"

        res = await buck.build(
            "//:write_tset", "-c", "buck2.write_action_cache_populate=1", *configs
        )
        if await soft_errors(buck, "write_action_cache_populate_failed"):
            output = res.get_build_report().output_for_target("//:write_tset")
            assert output.read_text() == expected
            return
        await buck.kill()

        # The projection's hash is memoized on the frozen set in each daemon; a hit across daemons
        # says it comes out the same.
        target = "//:write_tset_again"
        res = await buck.build(
            target,
            "--materializations=none",
            "-c",
            "buck2.write_action_cache=use",
            *configs,
        )
        output = res.get_build_report().output_for_target(target)
        assert counter(res, "write_action_cache_hits") >= 1
        assert (
            await get_last_execution_kind(buck, category="write")
            == ACTION_EXECUTION_KIND_DEFERRED
        )
        assert not output.exists()

        await buck.build(target, "-c", "buck2.write_action_cache=use", *configs)
        assert output.read_text() == expected

    @buck_test(write_invocation_record=True)
    async def test_a_key_outside_the_share_is_a_plain_write(self, buck: Buck) -> None:
        content = random_string()
        # A share this small holds none of the keys a test produces; a write outside it, with the
        # mode off, never asks the cache.
        res = await buck.build(
            "//:write",
            "-c",
            f"test.content={content}",
            "-c",
            "buck2.write_action_cache=off",
            "-c",
            "buck2.write_action_cache_populate=0.000000001",
        )
        output = res.get_build_report().output_for_target("//:write")
        assert counter(res, "write_action_cache_hits") == 0
        assert counter(res, "write_action_cache_misses") == 0
        assert output.read_text() == content

    @buck_test(write_invocation_record=True)
    async def test_nothing_is_recorded_where_local_results_are_not_published(
        self, buck: Buck
    ) -> None:
        content = random_string()
        configs = [
            "-c",
            f"test.content={content}",
            "-c",
            "test.allow_cache_uploads=false",
            "-c",
            "buck2.write_action_cache_populate=1",
        ]

        # The share covers every key, but the executor publishes no local results: with the mode
        # off the cache is not asked...
        res = await buck.build(
            "//:write", "-c", "buck2.write_action_cache=off", *configs
        )
        assert counter(res, "write_action_cache_misses") == 0
        assert (
            res.get_build_report().output_for_target("//:write").read_text() == content
        )

        # ...and under `use` it is asked, and the miss is not recorded.
        res = await buck.build(
            "//:write_again", "-c", "buck2.write_action_cache=use", *configs
        )
        assert counter(res, "write_action_cache_misses") >= 1
        assert counter(res, "write_action_cache_populated") == 0
        assert counter(res, "write_action_cache_populate_failures") == 0
        assert (
            res.get_build_report().output_for_target("//:write_again").read_text()
            == content
        )

    @buck_test()
    async def test_keys_are_golden(self, buck: Buck) -> None:
        dump = buck.cwd / "keys.txt"
        await buck.build(
            "//:write",
            "//:write_tset",
            "-c",
            "test.content=golden",
            "-c",
            "buck2.write_action_cache=verify",
            env={"BUCK2_TEST_WRITE_ACTION_CACHE_KEY_DUMP": str(dump)},
        )
        keys = {}
        for line in dump.read_text().splitlines():
            parts = line.split("\t")
            # `<owner>\t<output>\t<key>`; the owner displays as `prelude//:write (<configuration>)`.
            keys[parts[0].split(" (")[0]] = parts[-1]
        assert keys == GOLDEN_KEYS, dump.read_text()
