# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load(
    ":erlang_error_handler.bzl",
    "erlang_extract_otp_app_error_handler",
)
load(
    ":erlang_info.bzl",
    "ErlangOTPBinariesInfo",
    "ErtsToolchainInfo",
    "Tool",
)
load(
    ":erlang_otp_application.bzl",
    "otp_applications",
)
load(
    ":erlang_paths.bzl",
    "strip_extension",
)
load(
    ":erlang_toolchain.bzl",
    "default_toolchain_script_args_post",
    "default_toolchain_script_args_pre",
)

# The extraction runs `erl`, so its exec platform has to match the target platform or the
# artifacts come out for the wrong one. There is no DEFAULT: a target platform that is not
# listed here fails to configure rather than silently producing a foreign-architecture release.
_DEFAULT_EXEC_COMPATIBLE_WITH = select({
    "config//os/constraints:os[unspecified]": [],
    "config//os:linux": select({
        "config//cpu:arm64": [
            "config//cpu/constraints:arm64",
            "config//os/constraints:linux",
        ],
        "config//cpu:x86_64": [
            "config//cpu/constraints:x86_64",
            "config//os/constraints:linux",
        ],
    }),
    "config//os:macos": select({
        "config//cpu:arm64": [
            "config//cpu/constraints:arm64",
            "config//os/constraints:macos",
        ],
        "config//cpu:x86_64": [
            "config//cpu/constraints:x86_64",
            "config//os/constraints:macos",
        ],
    }),
    "config//os:windows": select({
        "config//cpu:arm64": [
            "config//cpu/constraints:arm64",
            "config//os/constraints:windows",
        ],
        "config//cpu:x86_64": [
            "config//cpu/constraints:x86_64",
            "config//os/constraints:windows",
        ],
    }),
})

def _extractor(ctx: AnalysisContext, binaries: ErlangOTPBinariesInfo, env: dict[str, str]) -> Tool:
    src = ctx.attrs._extract_from_otp
    name = strip_extension(src.basename)
    beam = ctx.actions.declare_output(name, name + ".beam", has_content_based_path = False)
    erlc = cmd_args(binaries.erlc, hidden = binaries.erl)
    ctx.actions.run(
        cmd_args(erlc, "+deterministic", "-o", cmd_args(beam.as_output(), parent = 1), src),
        category = "erlc",
        identifier = src.short_path,
        env = env,
    )
    return cmd_args(
        binaries.erl,
        cmd_args(beam, parent = 1, prepend = "-pa"),
        default_toolchain_script_args_pre,
        cmd_args(name, ":main(init:get_plain_arguments())", delimiter = ""),
        default_toolchain_script_args_post,
    )

def _erlang_erts_impl(ctx: AnalysisContext) -> list[Provider]:
    binaries = ctx.attrs.otp_binaries[ErlangOTPBinariesInfo]
    env = ctx.attrs.env
    extractor = _extractor(ctx, binaries, env)

    def extract(name: str, args: list[str], category: str, dir: bool = True) -> Artifact:
        out = ctx.actions.declare_output(name, dir = dir, has_content_based_path = False)
        ctx.actions.run(
            cmd_args(extractor, args, out.as_output()),
            category = category,
            identifier = ctx.attrs.name,
            env = env,
        )
        return out

    # one action per application, so a release only ever extracts what its own closure names
    applications = {}
    for application in otp_applications:
        out = ctx.actions.declare_output(paths.join("lib", application), dir = True, has_content_based_path = False)
        ctx.actions.run(
            cmd_args(extractor, paths.join("lib", application + "-*"), out.as_output()),
            category = "extract_otp_app",
            identifier = application,
            env = env,
            error_handler = erlang_extract_otp_app_error_handler,
        )
        applications[application] = out

    return [
        DefaultInfo(),
        ErtsToolchainInfo(
            applications = applications,
            erts = extract("erts", ["extract_into", "erts-*"], "extract_erts"),
            headers = extract("erts_headers", ["erts-*/include"], "extract_erts_headers"),
            otp_no_dot_erlang_boot = extract("otp_no_dot_erlang_boot", ["bin/no_dot_erlang.boot"], "extract_otp_boot", False),
            otp_start_boot = extract("otp_start_boot", ["bin/start.boot"], "extract_otp_start_boot", False),
            versions = extract("versions.json", ["versions"], "introspect_otp_versions", False),
        ),
    ]

_erlang_erts_rule = rule(
    impl = _erlang_erts_impl,
    attrs = {
        "env": attrs.dict(key = attrs.string(), value = attrs.string(), default = {}),
        "otp_binaries": attrs.toolchain_dep(),
        "_extract_from_otp": attrs.default_only(attrs.source(default = "prelude//erlang/erts:extract_from_otp.erl")),
    },
)

def erlang_erts(exec_compatible_with = _DEFAULT_EXEC_COMPATIBLE_WITH, **kwargs):
    _erlang_erts_rule(exec_compatible_with = exec_compatible_with, **kwargs)
