# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load(
    "@prelude//python_bootstrap:python_bootstrap.bzl",
    python_bootstrap_binary_spec = "python_bootstrap_binary",
    python_bootstrap_binary_stage0_spec = "python_bootstrap_binary_stage0",
    python_bootstrap_library_spec = "python_bootstrap_library",
)
load(":cxx_python_extension.bzl", cxx_python_extension_spec = "cxx_python_extension")
load(":prebuilt_python_library.bzl", prebuilt_python_library_spec = "prebuilt_python_library")
load(":python_binary.bzl", python_binary_spec = "python_binary")
load(":python_library.bzl", python_library_spec = "python_library")
load(":python_needed_coverage_test.bzl", python_needed_coverage_test_spec = "python_needed_coverage_test")
load(":python_runtime_bundle.bzl", python_runtime_bundle_spec = "python_runtime_bundle")
load(":python_test.bzl", python_test_spec = "python_test")

specs = {
    "cxx_python_extension": cxx_python_extension_spec,
    "prebuilt_python_library": prebuilt_python_library_spec,
    "python_binary": python_binary_spec,
    "python_bootstrap_binary": python_bootstrap_binary_spec,
    "python_bootstrap_binary_stage0": python_bootstrap_binary_stage0_spec,
    "python_bootstrap_library": python_bootstrap_library_spec,
    "python_library": python_library_spec,
    "python_needed_coverage_test": python_needed_coverage_test_spec,
    "python_runtime_bundle": python_runtime_bundle_spec,
    "python_test": python_test_spec,
}

rules = make_rules(specs)

load_symbols(rules)
