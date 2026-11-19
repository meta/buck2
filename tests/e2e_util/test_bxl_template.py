# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import os

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.api.buck_result import BuckException
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


class BxlTemplateTest(BuckTestCase):
    # This is just a template test case for `bxl_test` to use buck2's e2e test framework.
    # It does not need to be edited for new `bxl_test`.

    @buck_test(inplace=True)
    async def test_bxl(self, buck: Buck) -> None:
        args = []

        buck_args = os.environ.get("BUCK_ARGS")
        if buck_args:
            args += buck_args.split(" ")

        bxl_args = os.environ.get("BXL_ARGS")
        if bxl_args:
            args += ["--"] + bxl_args.split(" ")

        try:
            await buck.bxl(os.environ["BXL_MAIN"], *args)
        except BuckException as e:
            # Re-raise with stderr included in the message for better test output
            raise AssertionError(f"BXL failed:\n{e.stderr}") from e
