# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@fbcode_macros//build_defs:python_unittest.bzl", "python_unittest")
load("@fbsource//tools/build_defs/testinfra:network_access_utils.bzl", "network_access_utils")
load("@prelude//:is_full_meta_repo.bzl", "is_full_meta_repo")

def e2e_bin_smoke_test():
    # The test runs an fbcode-built binary; open source has no `fbcode//`
    # cell to locate it with, so only define the test in a full meta repo.
    if not is_full_meta_repo():
        return
    # Guards `buck2_e2e-bin`: the test binary must build, start, and report `--version`.
    python_unittest(
        name = "test_e2e_bin_smoke",
        srcs = ["test_e2e_bin_smoke.py"],
        env = {
            "BUCK2_E2E_BIN": "$(location fbcode//buck2/app/buck2:buck2_e2e-bin)",
        },
        network_access = network_access_utils.none(),
    )
