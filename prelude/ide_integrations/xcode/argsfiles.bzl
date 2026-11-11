# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# @oss-disable[end= ]: load("@prelude//apple/meta_only:xcode_argsfiles.bzl", "get_meta_specific_xcode_arg_substitutions")

def _get_meta_specific_xcode_arg_substitutions():
    # @oss-disable[end= ]: return get_meta_specific_xcode_arg_substitutions()
    return [] # @oss-enable

XCODE_ARGSFILES_SUB_TARGET = "xcode-argsfiles"

# The compile flags of the Xcode argsfiles as JSON, by source extension and contributing layer
# (`get_xcode_compiler_flags_output`): what a project generator reads instead of recomposing
# the flags from the attributes.
XCODE_COMPILER_FLAGS_SUB_TARGET = "xcode-compiler-flags"

XCODE_ARG_SUBSTITUTIONS = [
    (regex("-filter-error=.+"), "-fcolor-diagnostics"),
    (regex("-filter-ignore=.+"), "-fcolor-diagnostics"),
    (regex("-filter-warning=.+"), "-fcolor-diagnostics"),
] + _get_meta_specific_xcode_arg_substitutions()
