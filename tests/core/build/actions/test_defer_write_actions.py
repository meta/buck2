# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

"""
With `buck2.defer_write_actions` on, a write action's content stays off its output path until
something needs the file: the deferred core keeps the write as a declaration, the disk-state core
keeps the content in the BlobStore's memory, compressed. Under either, the write counts as
executed, a build that materializes nothing leaves nothing on disk, and the file is written when
a build asks for it or an action needs it, on disk here or in the CAS for a remote run.
"""

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.api.buck_result import BuckResult
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase
from buck2.tests.e2e_util.helper.utils import get_last_execution_kind, random_string

ACTION_EXECUTION_KIND_SIMPLE = 4


def counter(res: BuckResult, name: str) -> int:
    """A daemon counter at the end of the command, from its last snapshot."""
    return int(res.invocation_record()["last_snapshot"].get(name, 0))


class DeferWriteActionsTest(BuckTestCase):
    @buck_test(write_invocation_record=True)
    async def test_a_deferred_write_is_written_when_something_needs_it(
        self, buck: Buck
    ) -> None:
        content = random_string()
        configs = ["-c", f"test.content={content}"]

        # The write ran, and nothing is on disk...
        res = await buck.build("//:write", "--materializations=none", *configs)
        output = res.get_build_report().output_for_target("//:write")
        assert (
            await get_last_execution_kind(buck, category="write")
            == ACTION_EXECUTION_KIND_SIMPLE
        )
        assert not output.exists()

        # ...until something needs the file.
        await buck.build("//:write", *configs)
        assert output.read_text() == content

    @buck_test(write_invocation_record=True, skip_for_materializer=["deferred"])
    async def test_the_disk_state_core_keeps_the_content_in_memory(
        self, buck: Buck
    ) -> None:
        # Repetitive, so that compressing it shows.
        content = random_string()[:16] * 64
        configs = ["-c", f"test.content={content}"]

        res = await buck.build("//:write", "--materializations=none", *configs)
        output = res.get_build_report().output_for_target("//:write")
        assert counter(res, "blob_store_memory_entries") == 1
        assert 0 < counter(res, "blob_store_memory_bytes") < len(content)
        assert counter(res, "blob_store_entries") == 0, "nothing entered the stash"
        assert not output.exists()

        res = await buck.build("//:write", *configs)
        assert counter(res, "blob_store_memory_served_files") == 1
        assert output.read_text() == content

    @buck_test(write_invocation_record=True)
    async def test_a_deferred_write_is_uploaded_for_a_remote_action(
        self, buck: Buck
    ) -> None:
        content = random_string()
        configs = ["-c", f"test.content={content}"]

        res = await buck.build(
            "//:consume", "--remote-only", "--no-remote-cache", *configs
        )
        output = res.get_build_report().output_for_target("//:consume")
        assert output.read_text() == content

    @buck_test(write_invocation_record=True)
    async def test_a_deferred_write_is_on_disk_for_a_local_action(
        self, buck: Buck
    ) -> None:
        content = random_string()
        configs = ["-c", f"test.content={content}"]

        res = await buck.build("//:consume", "--local-only", *configs)
        output = res.get_build_report().output_for_target("//:consume")
        assert output.read_text() == content
