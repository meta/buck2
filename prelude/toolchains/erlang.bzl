# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//erlang:erlang_erts.bzl", "erlang_erts")
load("@prelude//erlang:erlang_info.bzl", "ErlangOTPBinariesInfo")
load("@prelude//erlang:erlang_toolchain.bzl", "erlang_toolchain")

def _system_erlang_binary_impl(_ctx):
    return [
        DefaultInfo(),
        ErlangOTPBinariesInfo(
            erl = cmd_args("erl"),
            erlc = cmd_args("erlc"),
            escript = cmd_args("escript"),
        ),
    ]

system_erlang_binary = rule(
    impl = _system_erlang_binary_impl,
    attrs = {},
    is_toolchain_rule = True,
)

def system_erlang_toolchain(name, visibility):
    system_erlang_binary(
        name = "{}-binaries".format(name),
        visibility = visibility,
    )

    erlang_erts(
        name = "{}-erts".format(name),
        otp_binaries = ":{}-binaries".format(name),
        visibility = visibility,
    )

    erlang_toolchain(
        name = name,
        erts_toolchain_info = ":{}-erts".format(name),
        otp_binaries = ":{}-binaries".format(name),
        parse_transforms = [],
        parse_transforms_filters = {},
        visibility = ["PUBLIC"],
    )
