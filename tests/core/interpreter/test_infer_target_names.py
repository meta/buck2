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
from buck2.tests.e2e_util.helper.golden import golden

# Turns `//lib/greeting` into `//lib/greeting:greeting` during coercion in
# build/bzl files. Off by default.
_ENABLE = "buck2.infer_target_names=true"


class InferTargetNamesTest(BuckTestCase):
    @buck_test()
    async def test_eponymous_dep_in_build_file_rejected_by_default(
        self, buck: Buck
    ) -> None:
        # `//:consumer` depends on the abbreviated `//lib/greeting`. Without the
        # buckconfig this is a coercion error, matching historical behavior.
        res = await expect_failure(
            buck.uquery("deps(root//:consumer)", "--console=none", "-v0"),
        )
        golden(
            output=res.stderr,
            rel_path="golden/eponymous_dep_rejected.golden.stderr",
        )

    @buck_test()
    async def test_eponymous_dep_in_build_file_inferred_when_enabled(
        self, buck: Buck
    ) -> None:
        # With the buckconfig, `//lib/greeting` is inferred to be
        # `//lib/greeting:greeting`, which is a real target, so the query succeeds.
        res = await buck.uquery("deps(root//:consumer)", "-c", _ENABLE)
        assert "root//lib/greeting:greeting" in res.stdout

    @buck_test()
    async def test_eponymous_source_in_build_file_rejected_by_default(
        self, buck: Buck
    ) -> None:
        # `attrs.source()` takes either a path or a label, so it coerces separately
        # from `attrs.dep()`. Without the buckconfig, `//lib/greeting` fails as both
        # a label without an explicit target name and as a non-relative path.
        await expect_failure(
            buck.uquery("deps(root//source:src_consumer)", "--console=none", "-v0"),
            stderr_regex=r"Couldn't coerce `//lib/greeting` as a source",
        )

    @buck_test()
    async def test_eponymous_source_in_build_file_inferred_when_enabled(
        self,
        buck: Buck,
    ) -> None:
        # With the buckconfig, the source attr coerces to the eponymous label rather
        # than being (mis)treated as a path.
        res = await buck.uquery("deps(root//source:src_consumer)", "-c", _ENABLE)
        assert "root//lib/greeting:greeting" in res.stdout

    @buck_test()
    async def test_eponymous_attr_default_in_bzl_rejected_by_default(
        self, buck: Buck
    ) -> None:
        # The `//bzl_default` rule declares `attrs.dep(default = "//lib/greeting")`.
        # The default is coerced while evaluating the .bzl module, so it errors
        # without the buckconfig.
        res = await expect_failure(
            buck.uquery("root//bzl_default:", "--console=none", "-v0"),
        )
        golden(
            output=res.stderr,
            rel_path="golden/eponymous_attr_default_rejected.golden.stderr",
        )

    @buck_test()
    async def test_eponymous_attr_default_in_bzl_inferred_when_enabled(
        self,
        buck: Buck,
    ) -> None:
        # With the buckconfig, the eponymous attr default resolves and shows up as a
        # dependency of the target.
        res = await buck.uquery("deps(root//bzl_default:target)", "-c", _ENABLE)
        assert "root//lib/greeting:greeting" in res.stdout

    @buck_test()
    async def test_eponymous_query_literal_inferred_when_enabled(
        self, buck: Buck
    ) -> None:
        # Query literals use a separate parser from attributes in build files. It must
        # honor the same setting so tools can feed abbreviated labels back to Buck.
        res = await buck.uquery("deps(root//lib/greeting)", "-c", _ENABLE)
        assert "root//lib/greeting:greeting" in res.stdout

    @buck_test()
    async def test_bxl_query_filter_infers_target_name_when_enabled(
        self, buck: Buck
    ) -> None:
        res = await buck.bxl("//:infer_target_names.bxl:main", "-c", _ENABLE)
        assert res.stdout.strip() == "[root//foo:foo]"
