# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//rust:arch_action.bzl", "rust_arch_program_action")
load(
    "@prelude//rust:rust_toolchain.bzl",
    "RustToolchainInfo",
    "get_rust_arch_toolchain_info",
)

def _get_rustc_cfg_impl(ctx: AnalysisContext) -> list[Provider]:
    toolchain = ctx.attrs._rust_toolchain
    toolchain_info = toolchain[RustToolchainInfo]
    arch_info = get_rust_arch_toolchain_info(toolchain, ctx.label)

    out = ctx.actions.declare_output("rustc.cfg", has_content_based_path = True)

    args = cmd_args(
        cmd_args("--print=cfg=", out.as_output(), delimiter = ""),
        cmd_args("--sysroot="),  # We do not need a sysroot here, and not all platforms we support have one available (e.g. mips64-unknown-linux-gnuabi64)
        toolchain_info.rustc_flags,
    )

    uses_custom_target = toolchain_info.rust_target_path != None

    if toolchain_info.rustc_target_triple:
        args.add(cmd_args("--target=", toolchain_info.rustc_target_triple, delimiter = ""))
        if uses_custom_target:
            args.add("-Zunstable-options")

    rule_env = {}
    if toolchain_info.nightly_features or ctx.attrs.enable_nightly_cfgs or uses_custom_target:
        rule_env["RUSTC_BOOTSTRAP"] = "1"

    if toolchain_info.rust_target_path != None:
        rule_env["RUST_TARGET_PATH"] = toolchain_info.rust_target_path[DefaultInfo].default_outputs[0]

    if arch_info == None:
        cmd = cmd_args(toolchain_info.compiler, args)
        ctx.actions.run(cmd, category = "rustc_cfg", env = rule_env)
    else:
        arch_programs = {}
        for arch, variant in arch_info.arch_variants.items():
            env = variant.plain_env | variant.path_env | rule_env
            cmd = cmd_args(
                "/usr/bin/env",
                [cmd_args(key, "=", value, delimiter = "") for key, value in env.items()],
                variant.compiler,
                args,
                hidden = variant.runtime_and_plugin_inputs,
            )
            arch_programs[arch] = RunInfo(args = cmd)
        ctx.actions.run(
            rust_arch_program_action(
                actions = ctx.actions,
                path = "rustc-cfg-arch-action",
                archs = arch_programs,
            ),
            category = "rustc_cfg",
        )

    return [DefaultInfo(default_output = out)]

get_rustc_cfg = rule(
    impl = _get_rustc_cfg_impl,
    attrs = {
        "enable_nightly_cfgs": attrs.bool(default = False),
        "_rust_toolchain": toolchains_common.rust(),
    },
)

def _get_rustc_host_tuple_impl(ctx: AnalysisContext) -> list[Provider]:
    toolchain = ctx.attrs._rust_toolchain
    toolchain_info = toolchain[RustToolchainInfo]
    if get_rust_arch_toolchain_info(toolchain, ctx.label) != None:
        fail(
            "{} targets a dual-arch toolchain, which has no single host tuple; consume the per-arch RustArchToolchainVariant.arch_triple instead".format(
                ctx.label
            )
        )

    out = ctx.actions.declare_output("rustc.host_tuple", has_content_based_path = True)

    cmd = [
        toolchain_info.compiler,
        cmd_args("--print=host-tuple=", out.as_output(), delimiter = ""),
    ]

    ctx.actions.run(cmd, category = "rustc_host_tuple")

    return [DefaultInfo(default_output = out)]

get_rustc_host_tuple = rule(
    impl = _get_rustc_host_tuple_impl,
    attrs = {
        "_rust_toolchain": toolchains_common.rust(),
    },
)

def _linkable_symbol_supports_no_std_impl(ctx: AnalysisContext) -> list[Provider]:
    toolchain_info = ctx.attrs._rust_toolchain[RustToolchainInfo]

    # `#[no_std]` requires use of `advanced_unstable_linking` on the toolchain,
    # as otherwise the panic handler is missing.
    cfg = "--cfg=set_nostd\n" if toolchain_info.advanced_unstable_linking else ""

    flagfile = ctx.actions.write("cfg", cfg, has_content_based_path = True)
    return [DefaultInfo(default_output = flagfile)]

linkable_symbol_supports_no_std = rule(
    impl = _linkable_symbol_supports_no_std_impl,
    attrs = {
        "_rust_toolchain": toolchains_common.rust(),
    },
)
