# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load(":julia_binary.bzl", julia_binary_spec = "julia_binary")
load(":julia_library.bzl", julia_jll_library_spec = "julia_jll_library", julia_library_spec = "julia_library")
load(":julia_test.bzl", julia_test_spec = "julia_test")

specs = {
    "julia_binary": julia_binary_spec,
    "julia_jll_library": julia_jll_library_spec,
    "julia_library": julia_library_spec,
    "julia_test": julia_test_spec,
}

rules = make_rules(specs)

load_symbols(rules)
