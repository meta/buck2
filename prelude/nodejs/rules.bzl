# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load(":nodejs_binary.bzl", nodejs_binary_spec = "nodejs_binary")
load(":nodejs_build.bzl", nodejs_build_spec = "nodejs_build")
load(":nodejs_library.bzl", nodejs_library_spec = "nodejs_library")
load(":nodejs_node_modules.bzl", nodejs_node_modules_spec = "nodejs_node_modules")
load(":nodejs_runtime.bzl", "nodejs_runtime")
load(":nodejs_test.bzl", nodejs_test_spec = "nodejs_test")

specs = {
    "nodejs_binary": nodejs_binary_spec,
    "nodejs_build": nodejs_build_spec,
    "nodejs_library": nodejs_library_spec,
    "nodejs_node_modules": nodejs_node_modules_spec,
    "nodejs_runtime": nodejs_runtime,
    "nodejs_test": nodejs_test_spec,
}

rules = make_rules(specs)

load_symbols(rules)
