# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

DEVSERVER_PLATFORM_REGEX = "UNUSED"
ANDROID = "Android"
APPLE = "Apple"
APPLETVOS = "appletvos"
CXX = "Default"
FBCODE = "Fbcode"
IOS = "ios"
MACOSX = "macosx"
WATCHOS = "watchos"
WINDOWS = "Windows"

def get_available_platforms():
    if native.host_info().os.is_macos:
        return (CXX, ANDROID, APPLE)
    if native.host_info().os.is_windows:
        return (CXX, ANDROID, WINDOWS)
    return (CXX, ANDROID)
