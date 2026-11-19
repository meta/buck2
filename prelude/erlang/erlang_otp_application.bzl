# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(":erlang_info.bzl", "ErlangAppInfo", "ErlangAppOrTestInfo")
load(
    ":erlang_toolchain.bzl",
    "get_toolchain",
)

# This is a superset of all available OTP applications and needs to be manually updated
# if new applications make it into OTP. New applications will not be available until
# they are listed here.
otp_applications = [
    "asn1",
    "common_test",
    "compiler",
    "crypto",
    "debugger",
    "dialyzer",
    "diameter",
    "edoc",
    "eldap",
    "erl_docgen",
    "erl_interface",
    "erts",
    "et",
    "eunit",
    "ftp",
    "inets",
    "jinterface",
    "kernel",
    "megaco",
    "mnesia",
    "observer",
    "os_mon",
    "parsetools",
    "public_key",
    "reltool",
    "runtime_tools",
    "sasl",
    "snmp",
    "ssh",
    "ssl",
    "stdlib",
    "syntax_tools",
    "tftp",
    "tools",
    "wx",
    "xmerl",
]

def gen_otp_applications() -> None:
    for name in otp_applications:
        _erlang_otp_application_rule(name = name, visibility = ["PUBLIC"])
    return None

def normalize_application(name: str) -> str:
    """Try to translate OTP application names to internal targets so users can write
    `kernel` instead of `prelude//erlang/applications:kernel`
    """
    if ":" not in name:
        if name in otp_applications:
            # Known OTP application - convert to prelude target
            return "prelude//erlang/applications:{}".format(name)
        else:
            # Not a known OTP app - might be a typo or user forgot the ":"
            fail('Unknown OTP application "{app}". If this is not supposed to be an OTP application, did you mean ":{app}"?'.format(app = name))
    return name

def _erlang_otp_application_impl(ctx: AnalysisContext) -> list[Provider]:
    """virtual OTP application for referencing only"""

    toolchain = get_toolchain(ctx)

    return [
        DefaultInfo(),
        ErlangAppOrTestInfo(),
        ErlangAppInfo(
            name = ctx.attrs.name,
            version = "dynamic",
            beams = [],
            includes = [],
            dependencies = {},
            code_path_tset = None,
            start_dependencies = None,
            include_dir = None,
            virtual = True,
            app_folder = toolchain.erts_toolchain_info.applications.get(ctx.attrs.name) if toolchain.erts_toolchain_info else None,
        ),
    ]

_erlang_otp_application_rule = rule(
    impl = _erlang_otp_application_impl,
    attrs = {
        "_toolchain": attrs.toolchain_dep(default = "toolchains//:erlang-default"),
    },
)
