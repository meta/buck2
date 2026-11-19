# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

_MIN_SDK_VERSION = 19
_MAX_SDK_VERSION = 36

def get_min_sdk_version_constraint_value_name(min_sdk: int) -> str:
    return "min_sdk_version_{}".format(min_sdk)

def get_min_sdk_version_range() -> range:
    return range(_MIN_SDK_VERSION, _MAX_SDK_VERSION + 1)
