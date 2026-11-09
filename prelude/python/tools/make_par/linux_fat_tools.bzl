# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@fbcode//buck2/platform/execution:fat_binary.bzl", "linux_fat_binary")

def define_linux_fat_gen_lpar_bootstrap():
    linux_fat_binary(
        name = "linux-fat-gen_lpar_bootstrap",
        platform_exe = {
            "linux": {
                "arm64": (":gen_lpar_bootstrap", "fbcode//buck2/platform/execution:platform010-aarch64"),
                "x86_64": (":gen_lpar_bootstrap", "fbcode//buck2/platform/execution:platform010"),
            },
        },
        target_compatible_with = ["ovr_config//os:linux"],
        visibility = ["PUBLIC"],
    )
