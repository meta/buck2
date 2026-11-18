# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

RemoteTestExecutionToolchainInfo = provider(
    fields = [
        # The profile to use by default.
        "default_profile",
        # A dictionary of string names to pre-registered profiles.  Rules can
        # use the profile name to references these.
        "profiles",
        # A bool indicating whether the test suites executed by this toolchain
        # should be run in a bundle. This makes all tests in a suite run in
        # a single RE action as opposed to one action per test.
        "default_run_as_bundle",
        # Optional callback that updates and returns the supplied RE properties.
        # Executor conversion passes a mutable copy and consumes the result.
        "prepare_re_properties",
    ],
)
