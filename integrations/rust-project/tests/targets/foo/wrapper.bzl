# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//rust:rust_binary.bzl", _prelude_rust_test = "rust_test")

def _custom_rust_test_impl(ctx: AnalysisContext) -> list[Provider]:
    return _prelude_rust_test.impl(ctx)

custom_rust_test = rule(
    impl = _custom_rust_test_impl,
    attrs = _prelude_rust_test.attrs,
    uses_plugins = _prelude_rust_test.uses_plugins,
    supports_incoming_transition = _prelude_rust_test.supports_incoming_transition,
)
