# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:command_alias.bzl", "command_alias", "command_alias_impl", command_alias_decl = "command_alias_rule")
load("@prelude//os_lookup:defs.bzl", "OsLookup")

def _alias_rule(os_lookup):
    return rule(
        impl = command_alias_impl,
        attrs = command_alias_decl.attrs
        | {
            "_target_os_type": attrs.dep(default = "fbcode//buck2/tests/prelude/command_alias/architecture:" + os_lookup),
        },
    )

x86_alias = _alias_rule("os_x86")
arm_alias = _alias_rule("os_arm")
fat_alias = _alias_rule("os_fat")
mac_linux_alias = _alias_rule("os_mac_linux")

def _direct_alias_impl(ctx):
    output = command_alias(
        actions = ctx.actions,
        path = "direct_alias",
        target_os = ctx.attrs.target_os[OsLookup],
        base = {"Linux": {ctx.attrs.arch: ctx.attrs.exe[RunInfo]}},
        args = cmd_args(),
        env = {},
        labels = [],
    )
    return [output.output]

direct_alias = rule(
    impl = _direct_alias_impl,
    attrs = {
        "arch": attrs.string(),
        "exe": attrs.dep(),
        "target_os": attrs.dep(default = "fbcode//buck2/tests/prelude/command_alias/architecture:os_fat"),
    },
)

def _tool_impl(ctx):
    return [DefaultInfo(), RunInfo(args = ["bash", ctx.attrs.script, ctx.attrs.arch])]

tool = rule(
    impl = _tool_impl,
    attrs = {"arch": attrs.string(), "script": attrs.source()},
)
