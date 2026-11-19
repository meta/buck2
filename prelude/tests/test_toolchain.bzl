# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:asserts.bzl", "asserts")

TestToolchainInfo = provider(
    fields = {
        # Used to populate sanitizer field in test infra.
        "sanitizer": str | None,
    }
)

def _impl(_ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo(), TestToolchainInfo(sanitizer = None)]

noop_test_toolchain = rule(
    impl = _impl,
    attrs = {},
    is_toolchain_rule = True,
)

def test_toolchain_labels(test_toolchain: Dependency) -> list[str]:
    asserts.true(TestToolchainInfo in test_toolchain, "Expected a TestToolchainInfo provider")
    test_toolchain = test_toolchain[TestToolchainInfo]
    return [test_toolchain.sanitizer] if test_toolchain.sanitizer else []
