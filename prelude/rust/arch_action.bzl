# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:command_alias.bzl", "command_alias")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup", "ScriptLanguage")
load(":rust_toolchain.bzl", "RustArchRunInfo", "require_rust_arch_variants")

# One complete invocation of rustc_action.py (or a protocol-compatible tool).
# wrapper_argfile is a cmd_args so it retains every artifact written into it.
RustArchCommand = record(
    wrapper = field(RunInfo),
    wrapper_argfile = field(cmd_args),
)

def rust_arch_action(*, actions: AnalysisActions, path: str, arch_commands: dict[str, RustArchCommand]) -> cmd_args:
    require_rust_arch_variants(arch_commands, path)
    archs = {arch: RunInfo(args = cmd_args(command.wrapper, command.wrapper_argfile)) for arch, command in arch_commands.items()}
    return rust_arch_program_action(actions = actions, path = path, archs = archs)

def rust_arch_program_action(*, actions: AnalysisActions, path: str, archs: dict[str, RunInfo]) -> cmd_args:
    require_rust_arch_variants(archs, path)
    output = command_alias(
        actions = actions,
        path = path,
        target_os = OsLookup(
            cpu = None,
            os = Os("fat_linux"),
            script = ScriptLanguage("sh"),
        ),
        base = {"Linux": archs},
        args = cmd_args(),
        env = {},
        labels = [],
        has_content_based_path = True,
    )
    return output.cmd

def _arch_run_aggregate_impl(ctx: AnalysisContext) -> list[Provider]:
    variants = require_rust_arch_variants(ctx.attrs.arch_variants, ctx.label)
    return [
        DefaultInfo(default_outputs = [output for dep in variants.values() for output in dep[DefaultInfo].default_outputs]),
        RustArchRunInfo(arch_variants = {arch: dep[RunInfo] for arch, dep in variants.items()}),
    ]

rust_arch_run_aggregate = rule(
    impl = _arch_run_aggregate_impl,
    attrs = {
        "arch_variants": attrs.dict(
            key = attrs.string(),
            value = attrs.dep(providers = [DefaultInfo, RunInfo]),
        ),
    },
)
