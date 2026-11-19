# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load("@prelude//python:rust_python_extension.bzl", rust_python_extension_spec = "rust_python_extension")
load(":rust_binary.bzl", rust_binary_spec = "rust_binary", rust_test_spec = "rust_test")
load(":rust_library.bzl", rust_library_spec = "rust_library")

specs = {
    "rust_binary": rust_binary_spec,
    "rust_library": rust_library_spec,
    "rust_python_extension": rust_python_extension_spec,
    "rust_test": rust_test_spec,
}

rules = make_rules(specs)

load_symbols(rules)
