# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import tempfile
from pathlib import Path
from unittest import mock

from buck2.tests.e2e_util import buck_workspace
from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


class RootBoundaryTest(BuckTestCase):
    async def asyncSetUp(self) -> None:
        await super().asyncSetUp()
        outer = Path(self.enterContext(tempfile.TemporaryDirectory()))
        (outer / ".buckconfig").write_text("[cells]\nroot = .\n")
        (outer / ".buckroot").touch()
        self.enterContext(
            mock.patch.object(buck_workspace, "_get_common_dir", return_value=outer)
        )

    @buck_test(skip_final_kill=True)
    async def test_root_stays_inside_fixture(self, buck: Buck) -> None:
        result = await buck.root("--kind=project")
        self.assertEqual(Path(result.stdout.strip()).resolve(), buck.cwd.resolve())
