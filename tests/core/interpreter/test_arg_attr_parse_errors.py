# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


class ArgAttrParseErrorsTest(BuckTestCase):
    @buck_test()
    async def test_malformed_macro_after_multibyte_char_is_an_error(
        self, buck: Buck
    ) -> None:
        """A malformed `attrs.arg()` macro is a coercion error that quotes the input, also when
        a multibyte character sits where the quoted window is cut."""
        await expect_failure(
            buck.targets("//:"),
            stderr_regex="Unfinished macro type.*\n.*V:\u00e9aaaaaaaaaaaa",
        )
        # The daemon is still there: the next command does not have to start a new one.
        result = await buck.targets("//ok:")
        assert "Starting new buck2 daemon" not in result.stderr, result.stderr
