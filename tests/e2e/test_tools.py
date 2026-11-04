# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


import re
import sys

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase, env


# builds targets in an fbcode target configuration, unsupported on mac RE workers
def fbcode_linux_only() -> bool:
    return sys.platform == "linux"


class ToolsTest(BuckTestCase):
    if fbcode_linux_only():

        @buck_test(inplace=True, skip_for_os=["windows"])
        async def test_swig_pp(self, buck: Buck) -> None:
            await buck.build(
                "fbcode//security/ca/lib:CAUtils-py-gen",
            )

        @buck_test(inplace=True)
        @env("BUCK2_KEEP_DEP_FILE_DIRECTORIES", "true")
        async def test_arvr_cuda_dep_files(self, buck: Buck) -> None:
            target = "fbsource//arvr/tools/buck/tests/cuda:test_cuda_arvr"
            mode_file = "@fbsource//arvr/mode/platform010/cuda12_8/opt"
            await buck.build(mode_file, target)
            res = await buck.audit_dep_files(
                target, "cuda_compile", "main.cu", mode_file
            )
            out = res.stdout

            # Check that we are tracking our dependency on stdlib headers, even
            # though they are neither explicitly included nor tagged.
            assert re.search(
                "untagged.*arvr/third-party/toolchains/platform010/build/glibc", out
            )

            # Check that we are tracking directly-included headers
            assert re.search("headers.*arvr/tools/buck/tests/cuda/direct_dep.h", out)

            # Check that we are tracking transitively-included headers
            assert re.search(
                "headers.*arvr/tools/buck/tests/cuda/transitive_dep.h", out
            )

            # Check that we are not tracking irrelevant headers
            assert (
                re.search("headers.*arvr/tools/buck/tests/cuda/unrelated_dep.h", out)
                is None
            )

    @buck_test(inplace=True, skip_for_os=["windows"])
    async def test_swig_pp_unit(self, buck: Buck) -> None:
        await buck.test(
            "fbcode//tools/build/buck:swig_filter_test",
        )

    @buck_test(inplace=True)
    async def test_windows_dummy(self) -> None:
        # None of the tests in this file pass on Windows and that upsets CI.
        pass
