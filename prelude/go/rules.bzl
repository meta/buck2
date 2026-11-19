# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load("@prelude//go_bootstrap:go_bootstrap.bzl", go_bootstrap_binary_spec = "go_bootstrap_binary")
load(":go_binary.bzl", go_binary_spec = "go_binary")
load(":go_exported_library.bzl", go_exported_library_spec = "go_exported_library")
load(":go_library.bzl", go_library_spec = "go_library")
load(":go_stdlib.bzl", go_stdlib_spec = "go_stdlib")
load(":go_test.bzl", go_test_spec = "go_test")

specs = {
    "go_binary": go_binary_spec,
    "go_bootstrap_binary": go_bootstrap_binary_spec,
    "go_exported_library": go_exported_library_spec,
    "go_library": go_library_spec,
    "go_stdlib": go_stdlib_spec,
    "go_test": go_test_spec,
}

rules = make_rules(specs)

load_symbols(rules)
