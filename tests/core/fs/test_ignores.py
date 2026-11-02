# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


import json

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.api.buck_result import BuckResult
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


def _srcs(res: BuckResult) -> list[str]:
    """The `srcs` of the one target printed with `--output-attribute srcs`, as project paths."""
    return [s.split("//", 1)[1].lstrip("/") for s in json.loads(res.stdout)[0]["srcs"]]


class IgnoresTest(BuckTestCase):
    # `.buckconfig` ignores `ignored` and every `*.gen` directory, and un-ignores parts of them
    # with `!` patterns. Reading an ignored path is a soft error, which fails any test not marked
    # `allow_soft_errors`: a build that passes read nothing ignored.

    @buck_test()
    async def test_unignored_directory(self, buck: Buck) -> None:
        # `!kept.gen` has no glob characters, so it covers the directory and everything in it.
        res = await buck.targets("//kept.gen:t", "--output-attribute", "srcs")
        self.assertEqual(_srcs(res), ["kept.gen/TARGETS.fixture", "kept.gen/sub/f"])

    @buck_test()
    async def test_unignored_file(self, buck: Buck) -> None:
        # `!shown/*.gen` and `!shown/*.gen/keep` un-ignore one file. The rest of the directory
        # stays ignored, in globs as in explicit reads.
        await buck.build("//:keep")
        res = await expect_failure(buck.build("//:other"))
        self.assertIn(
            "`shown/x.gen/other` matches the `project.ignore` pattern `**/*.gen/**`",
            res.stderr,
        )
        res = await buck.targets("//:globbed", "--output-attribute", "srcs")
        self.assertEqual(_srcs(res), ["shown/x.gen/keep"])

    @buck_test()
    async def test_unignored_file_changes_are_seen(self, buck: Buck) -> None:
        res = await buck.build_without_report("//:keep", "--out", "-")
        self.assertEqual(res.stdout.strip(), "KEEP")
        (buck.cwd / "shown" / "x.gen" / "keep").write_text("CHANGED")
        res = await buck.build_without_report("//:keep", "--out", "-")
        self.assertEqual(res.stdout.strip(), "CHANGED")

    @buck_test()
    async def test_no_unignoring_under_an_ignored_directory(self, buck: Buck) -> None:
        # `!ignored/inside` does nothing while `ignored` stays ignored.
        res = await expect_failure(buck.targets("//ignored/inside:"))
        self.assertIn(
            "dir `root//ignored` does not exist (project.ignore contains `ignored`)",
            res.stderr,
        )
