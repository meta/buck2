# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


class CellSegmentationTest(BuckTestCase):
    @buck_test()
    async def test_baseline_intra_cell(self, buck: Buck) -> None:
        await buck.build("//:intra_cell", "-cbuck2.disable_cell_segmentation=false")
        await buck.build("//:intra_cell", "-cbuck2.disable_cell_segmentation=true")

    @buck_test()
    async def test_fails_cross_cell_with_segmentation(self, buck: Buck) -> None:
        await expect_failure(
            buck.build("//:cross_cell", "-cbuck2.disable_cell_segmentation=false"),
            stderr_regex="Transitive set transitive values must be of the same transitive set type",
        )

    @buck_test()
    async def test_succeeds_cross_cell_without_segmentation(self, buck: Buck) -> None:
        await buck.build("//:cross_cell", "-cbuck2.disable_cell_segmentation=true")
