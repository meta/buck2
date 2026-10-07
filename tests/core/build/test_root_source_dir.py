# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


async def _list_files(buck: Buck, target: str) -> list[str]:
    result = await buck.build(target)
    out = result.get_build_report().output_for_target(target)
    return out.read_text().splitlines()


class RootSourceDirTest(BuckTestCase):
    @buck_test()
    async def test_root_source_dir_on_command_line(self, buck: Buck) -> None:
        result = await buck.build("root//:path")
        out = result.get_build_report().output_for_target("root//:path")
        assert out.read_text() == "."

        files = await _list_files(buck, "root//:list")
        assert "file" in files
        assert "sub/file" in files
        assert "TARGETS.fixture" in files

    @buck_test()
    async def test_root_source_dir_with_file_inside(self, buck: Buck) -> None:
        files = await _list_files(buck, "root//:list_with_file")
        assert "sub/file" in files

    @buck_test()
    async def test_root_source_dir_digest(self, buck: Buck) -> None:
        files = await _list_files(buck, "root//:list_copy")
        assert "file" in files
        assert "sub/file" in files
        assert "TARGETS.fixture" in files
        # `project.ignore` contains `ignored`. `other` is a separate cell. buck2 always ignores
        # `buck-out` in the root cell.
        for excluded in ["ignored", "other", "buck-out"]:
            assert not [f for f in files if f.split("/")[0] == excluded], files

    @buck_test()
    async def test_sub_source_dir(self, buck: Buck) -> None:
        assert await _list_files(buck, "root//:list_sub") == ["file"]
