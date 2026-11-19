# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "make_rules")
load(":erlang_application.bzl", erlang_app_spec = "erlang_app")
load(":erlang_application_includes.bzl", erlang_app_includes_spec = "erlang_app_includes")
load(":erlang_escript.bzl", erlang_escript_spec = "erlang_escript")
load(":erlang_release.bzl", erlang_release_spec = "erlang_release")
load(":erlang_tests.bzl", erlang_test_spec = "erlang_test")
load(":erlang_toolchain.bzl", erlang_otp_binaries_spec = "erlang_otp_binaries")

specs = {
    "erlang_app": erlang_app_spec,
    "erlang_app_includes": erlang_app_includes_spec,
    "erlang_escript": erlang_escript_spec,
    "erlang_otp_binaries": erlang_otp_binaries_spec,
    "erlang_release": erlang_release_spec,
    "erlang_test": erlang_test_spec,
}

rules = make_rules(specs)

load_symbols(rules)
