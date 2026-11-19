# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load(":bitcode.bzl", llvm_link_bitcode_spec = "llvm_link_bitcode")
load(
    ":cxx.bzl",
    cxx_binary_spec = "cxx_binary",
    cxx_library_spec = "cxx_library",
    cxx_precompiled_header_spec = "cxx_precompiled_header",
    cxx_test_spec = "cxx_test",
    prebuilt_cxx_library_spec = "prebuilt_cxx_library",
)
load(":cxx_flags.bzl", cxx_flags_spec = "cxx_flags")
load(":cxx_genrule.bzl", cxx_genrule_spec = "cxx_genrule")
load(":cxx_toolchain.bzl", cxx_toolchain_spec = "cxx_toolchain")
load(":prebuilt_cxx_library_group.bzl", prebuilt_cxx_library_group_spec = "prebuilt_cxx_library_group")
load(":transformation_spec.bzl", transformation_spec_spec = "transformation_spec")
load(":windows_resource.bzl", windows_resource_spec = "windows_resource")

specs = {
    "cxx_binary": cxx_binary_spec,
    "cxx_flags": cxx_flags_spec,
    "cxx_genrule": cxx_genrule_spec,
    "cxx_library": cxx_library_spec,
    "cxx_precompiled_header": cxx_precompiled_header_spec,
    "cxx_test": cxx_test_spec,
    "cxx_toolchain": cxx_toolchain_spec,
    "llvm_link_bitcode": llvm_link_bitcode_spec,
    "prebuilt_cxx_library": prebuilt_cxx_library_spec,
    "prebuilt_cxx_library_group": prebuilt_cxx_library_group_spec,
    "transformation_spec": transformation_spec_spec,
    "windows_resource": windows_resource_spec,
}

rules = make_rules(specs)

load_symbols(rules)
