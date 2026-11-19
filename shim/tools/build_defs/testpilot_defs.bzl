# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Stub `testpilot_defs.bzl` for the OSS buck2 build.
#
# `prelude/toolchains/android/test/com/facebook/buck/testrunner/BUCK`
# loads this file from `@fbsource//tools/build_defs:testpilot_defs.bzl` to
# get a `tpx_labels` struct (used as `labels = [tpx_labels.long_running]`).
# In the OSS shim, `fbsource = gh_facebook_buck2_shims_meta`, so the load
# resolves here. The fbcode-internal version provides Test Pilot label
# constants; in OSS we don't run via Test Pilot, so just expose the
# string literals the prelude references so that BUCK file parses.

tpx_labels = struct(
    heavyweight = "heavyweight",
    long_running = "long_running",
)
