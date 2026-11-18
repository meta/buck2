# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _get_cxx_platform_for_base_path(_base_path):
    return struct(target_platform = None)

def _get_cxx_platform_for_current_buildfile():
    return struct(target_platform = None)

platform_utils = struct(
    get_cxx_platform_for_base_path = _get_cxx_platform_for_base_path,
    get_cxx_platform_for_current_buildfile = _get_cxx_platform_for_current_buildfile,
)
