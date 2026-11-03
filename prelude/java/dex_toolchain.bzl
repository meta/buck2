# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# TODO(T107163344) These should be part of the Android toolchain!
# Move out once we have overlays.
DexToolchainInfo = provider(
    doc = "Dex toolchain info",
    fields = {
        "android_jar": provider_field(typing.Any, default = None),
        # Pre-dex also writes each class's ref counts for a library over a 64K limit.
        "class_ref_counts": provider_field(bool, default = False),
        "d8_command": provider_field(typing.Any, default = None),
    },
)
