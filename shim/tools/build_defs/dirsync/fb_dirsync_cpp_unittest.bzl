# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@fbsource//tools/build_defs:platform_defs.bzl", "CXX", "FBCODE", "xplat_target_compatible_with")
load("@shim//build_defs:cpp_unittest.bzl", "cpp_unittest")

def fb_dirsync_cpp_unittest(
    name,
    platforms = (CXX, FBCODE),
    apple_sdks = (),
    fbandroid_compiler_flags = None,
    fbandroid_labels = None,
    fbandroid_run_individual_tests_ait = None,
    fbobjc_compiler_flags = None,
    use_instrumentation_test = None,
    xplat_impl = None,
    fbcode_impl = None,
    use_raw_headers = False,
    xplat_labels = [],
    xplat_compiler_flags = [],
    labels = [],
    headers = [],
    **kwargs,
):
    _unused = (
        fbandroid_compiler_flags,
        fbandroid_labels,
        fbandroid_run_individual_tests_ait,
        fbobjc_compiler_flags,
        use_instrumentation_test,
        xplat_impl,
        fbcode_impl,
        use_raw_headers,
    )  # @unused
    if xplat_compiler_flags:
        kwargs["compiler_flags"] = kwargs.get("compiler_flags", []) + xplat_compiler_flags
    kwargs["headers"] = headers
    kwargs["target_compatible_with"] = (kwargs.pop("target_compatible_with", None) or []) + xplat_target_compatible_with(platforms, apple_sdks)
    cpp_unittest(name = name, labels = (labels or []) + (xplat_labels or []), **kwargs)
