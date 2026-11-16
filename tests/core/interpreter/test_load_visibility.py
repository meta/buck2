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


class LoadVisibilityTest(BuckTestCase):
    @buck_test()
    async def test_load_visibility_allowed(self, buck: Buck) -> None:
        """Loads that satisfy the declared `visibility()` are
        permitted: `//app/sub` is within `//app/...`, public/default `.bzl` files
        load from anywhere, and same-package loads (including of a private `.bzl`)
        always work."""
        await buck.targets("//...")

    @buck_test(data_dir="denied")
    async def test_load_visibility_denied(self, buck: Buck) -> None:
        """Loading a private (`visibility("private")`) `.bzl`
        from another package is rejected. The error shows the declared visibility
        as written and the pattern that would allow the load."""
        failure = await expect_failure(
            buck.targets("//other:"),
            stderr_regex="not visible to this package",
        )
        self.assertIn('declares `visibility("private")`', failure.stderr)
        self.assertIn(
            'its `visibility()` must include `"root//other:"`', failure.stderr
        )

    @buck_test(data_dir="denied")
    async def test_load_visibility_subpackage_denied(self, buck: Buck) -> None:
        """A subdirectory with its own buildfile is a different package, so it
        may not load its parent package's private `.bzl`."""
        failure = await expect_failure(
            buck.targets("//lib/sub:"),
            stderr_regex="not visible to this package",
        )
        self.assertIn(
            'its `visibility()` must include `"root//lib/sub:"`', failure.stderr
        )

    @buck_test(data_dir="denied")
    async def test_load_visibility_gate_off(self, buck: Buck) -> None:
        """The same cross-package private load is allowed when enforcement is
        disabled with `buck2.check_load_visibility=false`."""
        await buck.targets("//...", "-c", "buck2.check_load_visibility=false")

    @buck_test(data_dir="twice")
    async def test_load_visibility_at_most_once(self, buck: Buck) -> None:
        """`visibility()` may be called at most once per file. This is enforced at
        evaluation time, independent of the enforcement buckconfig."""
        await expect_failure(
            buck.targets("//..."),
            stderr_regex="at most once",
        )

    @buck_test(data_dir="indirect")
    async def test_load_visibility_must_be_direct(self, buck: Buck) -> None:
        """`visibility()` must be called directly in the `.bzl` file it applies to,
        not delegated to a loaded helper function."""
        await expect_failure(
            buck.targets("//..."),
            stderr_regex="must be called at the top level of the `.bzl` file",
        )

    @buck_test(data_dir="bad_usage")
    async def test_load_visibility_in_function(self, buck: Buck) -> None:
        """`visibility()` must be called at the top level, not from a function
        defined in the same file."""
        await expect_failure(
            buck.targets("//in_function:"),
            stderr_regex="must be called at the top level of the `.bzl` file",
        )

    @buck_test(data_dir="bad_usage")
    async def test_load_visibility_bad_patterns(self, buck: Buck) -> None:
        """Entries that are not package patterns are rejected with a hint."""
        for package, message in [
            (
                "target_label",
                "`//other:foo` is a target label, but `visibility()` takes package patterns (`//pkg:` or `//pkg/...`)",
            ),
            (
                "missing_colon",
                "`//other` is not a package pattern; `visibility()` takes package patterns (`//pkg:` or `//pkg/...`)",
            ),
            (
                "wrong_case",
                '`"Public"` is not valid in `visibility()`; use `"PUBLIC"` or `"public"`',
            ),
            (
                "negative",
                "Negative patterns are not supported in `visibility()`: `-//other/...`",
            ),
        ]:
            failure = await expect_failure(buck.targets(f"//{package}:"))
            self.assertIn(message, failure.stderr)

    @buck_test(data_dir="bxl")
    async def test_load_visibility_bxl_allowed(self, buck: Buck) -> None:
        """Load visibility of a `.bzl` is enforced for `.bxl` loaders too: a `.bxl`
        within the declared visibility may load the `.bzl`."""
        await buck.bxl("//app/allowed.bxl:check")

    @buck_test(data_dir="bxl")
    async def test_load_visibility_bxl_denied(self, buck: Buck) -> None:
        """A `.bxl` loading a private `.bzl` from another package is rejected."""
        await expect_failure(
            buck.bxl("//app/denied.bxl:check"),
            stderr_regex="not visible to this package",
        )

    @buck_test(data_dir="bxl")
    async def test_load_visibility_no_package_denied(self, buck: Buck) -> None:
        """A `.bxl` outside of any package may only load public `.bzl` files."""
        await expect_failure(
            buck.bxl("//nopkg/denied.bxl:check"),
            stderr_regex="is not in any package",
        )
