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
from buck2.tests.e2e_util.helper.golden import golden_replace_cfg_hash


class AuditExecutionPlatformResolutionTest(BuckTestCase):
    @buck_test()
    async def test_audit_execition_platform_resolution(self, buck: Buck) -> None:
        result = await buck.audit("execution-platform-resolution", "//:target")
        golden_replace_cfg_hash(output=result.stdout, rel_path="out.txt.golden")

    @buck_test()
    async def test_audit_execution_platform_resolution_no_compatible_platform(
        self,
        buck: Buck,
    ) -> None:
        result = await buck.audit(
            "execution-platform-resolution", "//:no_compatible_platform"
        )
        golden_replace_cfg_hash(
            output=result.stdout, rel_path="no_compatible_platform.txt.golden"
        )

    @buck_test()
    async def test_audit_execution_platform_resolution_error_fallback(
        self, buck: Buck
    ) -> None:
        # With `fallback = "error"` the target fails to configure outright, so there is no
        # retained resolution to read the skip reasons from; the audit command reconstructs
        # them by re-walking the candidates.
        result = await buck.audit(
            "execution-platform-resolution",
            "//:no_compatible_platform",
            "--setting",
            "execution_platforms.target=root//execution_platforms:execution_platforms_error_fallback",
        )
        golden_replace_cfg_hash(
            output=result.stdout, rel_path="error_fallback.txt.golden"
        )
