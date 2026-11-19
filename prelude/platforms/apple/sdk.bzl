# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

_APPLE_SDKS = [
    "iphoneos",
    "iphonesimulator",
    "maccatalyst",
    "macosx",
    "visionos",
    "visionsimulator",
    "watchos",
    "watchsimulator",
    "appletvos",
    "appletvsimulator",
    # Marker entry used to help toolchain selectors define a set of
    # tools outside the apple_toolchain definition.
    "toolchain-tool",
]

AppleSdk = enum(*_APPLE_SDKS)
