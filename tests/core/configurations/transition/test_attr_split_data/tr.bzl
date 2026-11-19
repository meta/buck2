# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl(platform, refs):
    _ = platform  # buildifier: disable=unused-variable
    cpu = refs.cpu
    arm64 = refs.arm64
    arm32 = refs.arm32
    return {
        "arm32": PlatformInfo(
            label = "arm32",
            configuration = ConfigurationInfo(
                constraints = {
                    cpu[ConstraintSettingInfo].label: arm32[ConstraintValueInfo],
                },
                values = {},
            ),
        ),
        "arm64": PlatformInfo(
            label = "arm64",
            configuration = ConfigurationInfo(
                constraints = {
                    cpu[ConstraintSettingInfo].label: arm64[ConstraintValueInfo],
                },
                values = {},
            ),
        ),
    }

cpu_split_transition = transition(
    impl = _impl,
    refs = {
        "arm32": "root//:arm32",
        "arm64": "root//:arm64",
        "cpu": "root//:cpu",
    },
    split = True,
)
