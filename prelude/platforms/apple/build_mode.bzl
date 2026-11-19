# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# @oss-disable[end= ]: load(
    # @oss-disable[end= ]: "@prelude//platforms/apple/meta_only:build_mode.bzl",
    # @oss-disable[end= ]: _APPLE_BUILD_MODES = "APPLE_BUILD_MODES",
    # @oss-disable[end= ]: _BUILD_MODE = "BUILD_MODE",
    # @oss-disable[end= ]: _REMAPPED_BUILD_MODES = "REMAPPED_BUILD_MODES",
    # @oss-disable[end= ]: _get_build_mode = "get_build_mode",
    # @oss-disable[end= ]: _get_build_mode_debug = "get_build_mode_debug",
    # @oss-disable[end= ]: _get_build_mode_release = "get_build_mode_release",
# @oss-disable[end= ]: )

BUILD_MODE_DEBUG = "debug" # @oss-enable
BUILD_MODE_PROFILE = "profile" # @oss-enable
BUILD_MODE_RELEASE = "release" # @oss-enable

APPLE_BUILD_MODES = [BUILD_MODE_DEBUG, BUILD_MODE_PROFILE, BUILD_MODE_RELEASE] # @oss-enable
# @oss-disable[end= ]: APPLE_BUILD_MODES = _APPLE_BUILD_MODES

# 1:1 mapping of build mode to canonical (supported/default) apple build modes
REMAPPED_BUILD_MODES = {} # @oss-enable
# @oss-disable[end= ]: REMAPPED_BUILD_MODES = _REMAPPED_BUILD_MODES

BUILD_MODE = struct(DEBUG = BUILD_MODE_DEBUG, PROFILE = BUILD_MODE_PROFILE, RELEASE = BUILD_MODE_RELEASE) # @oss-enable
# @oss-disable[end= ]: BUILD_MODE = _BUILD_MODE

CONSTRAINT_PACKAGE = "prelude//platforms/apple/constraints" # @oss-enable
# @oss-disable[end= ]: CONSTRAINT_PACKAGE = "ovr_config//build_mode/apple/constraints"

def get_build_mode():
    return read_root_config("apple", "build_mode", BUILD_MODE_DEBUG) # @oss-enable
    # @oss-disable[end= ]: return _get_build_mode()

def get_build_mode_debug():
    return BUILD_MODE.DEBUG # @oss-enable
    # @oss-disable[end= ]: return _get_build_mode_debug()

def get_build_mode_release():
    return BUILD_MODE.RELEASE # @oss-enable
    # @oss-disable[end= ]: return _get_build_mode_release()
