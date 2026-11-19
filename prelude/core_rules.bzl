# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:alias.bzl",
    alias_spec = "alias",
    configured_alias_spec = "configured_alias",
    toolchain_alias_spec = "toolchain_alias",
    versioned_alias_spec = "versioned_alias",
)
load("@prelude//:build_variants.bzl", build_variants_spec = "build_variants")
load("@prelude//:command_alias.bzl", command_alias_spec = "command_alias_rule")
load("@prelude//:export_file.bzl", export_file_spec = "export_file")
load("@prelude//:filegroup.bzl", filegroup_spec = "filegroup")
load("@prelude//:genrule.bzl", genrule_spec = "genrule")
load("@prelude//:http_file.bzl", http_file_spec = "http_file")
load("@prelude//:prelude_rule.bzl", "make_rules")
load("@prelude//:remote_file.bzl", remote_file_spec = "remote_file")
load("@prelude//:sh_binary.bzl", sh_binary_spec = "sh_binary")
load("@prelude//:sh_test.bzl", sh_test_spec = "sh_test")
load("@prelude//:test_suite.bzl", test_suite_spec = "test_suite")
load("@prelude//http_archive:http_archive.bzl", http_archive_spec = "http_archive")
load("@prelude//js:worker_tool.bzl", worker_tool_spec = "worker_tool")
load("@prelude//zip_file:zip_file.bzl", zip_file_spec = "zip_file")

# The rules that live at the prelude root (or close to it), documented under
# "core" (together with the configuration rules) and "shell".
specs = {
    "alias": alias_spec,
    "build_variants": build_variants_spec,
    "command_alias": command_alias_spec,
    "configured_alias": configured_alias_spec,
    "export_file": export_file_spec,
    "filegroup": filegroup_spec,
    "genrule": genrule_spec,
    "http_archive": http_archive_spec,
    "http_file": http_file_spec,
    "remote_file": remote_file_spec,
    "test_suite": test_suite_spec,
    "toolchain_alias": toolchain_alias_spec,
    "versioned_alias": versioned_alias_spec,
    "worker_tool": worker_tool_spec,
    "zip_file": zip_file_spec,
}

shell_specs = {
    "sh_binary": sh_binary_spec,
    "sh_test": sh_test_spec,
}

rules = make_rules(specs)

shell_rules = make_rules(shell_specs)

load_symbols(rules)

load_symbols(shell_rules)
