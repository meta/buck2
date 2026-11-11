# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:command_alias.bzl", "command_alias")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup", "ScriptLanguage")
load(":rust_toolchain.bzl", "require_rust_arch_variants")

# One complete invocation of rustc_action.py (or a protocol-compatible tool).
# wrapper_argfile is a cmd_args so it retains every artifact written into it.
RustArchCommand = record(
    wrapper = field(RunInfo),
    wrapper_argfile = field(cmd_args),
)

def rust_arch_action(*, actions: AnalysisActions, path: str, arch_commands: dict[str, RustArchCommand]) -> cmd_args:
    require_rust_arch_variants(arch_commands, path)
    branches = {arch: RunInfo(args = cmd_args(command.wrapper, command.wrapper_argfile)) for arch, command in arch_commands.items()}
    output = command_alias(
        actions = actions,
        path = path,
        target_os = OsLookup(
            cpu = None,
            os = Os("fat_linux"),
            script = ScriptLanguage("sh"),
        ),
        base = {"Linux": branches},
        args = cmd_args(),
        env = {},
        labels = [],
        has_content_based_path = True,
    )
    return output.cmd
