# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


class ProjectIgnorePatternsTest(BuckTestCase):
    @buck_test()
    async def test_plain_ignore_name_with_a_closing_brace(self, buck: Buck) -> None:
        """A `project.ignore` entry that is not a glob is matched literally even when it
        contains a glob metacharacter such as `}`; it must not break the cell's file
        operations."""
        with open(buck.cwd / ".buckconfig", "a") as buckconfig:
            buckconfig.write("[project]\nignore = a}\n")
        result = await buck.targets("//:")
        assert result.stdout.strip() == "root//:t"
