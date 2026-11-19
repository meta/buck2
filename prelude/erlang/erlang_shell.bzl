# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(":erlang_build.bzl", "erlang_build")
load(":erlang_dependencies.bzl", "erlang_deps_rule")
load(":erlang_info.bzl", "ErlangDependencyInfo", "code_path_args")
load(":erlang_toolchain.bzl", "get_toolchain")

def _build_run_info(
    ctx: AnalysisContext,
    *,
    dep_info: ErlangDependencyInfo,
    additional_code_path: cmd_args,
    additional_shell_deps: list[Dependency] = [],
    additional_args: [cmd_args, None] = None,
) -> Promise:
    """Builds an Erlang shell with the dependencies and additional code paths available."""
    shell_dep_info = ctx.actions.anon_target(erlang_deps_rule, {"deps": additional_shell_deps + ctx.attrs.shell_libs})

    return shell_dep_info.promise.map(
        lambda shell_dep_info: _do_build_run_info(
            ctx,
            dep_info,
            additional_code_path,
            additional_args,
            shell_dep_info[ErlangDependencyInfo],
        )
    )

def _do_build_run_info(
    ctx: AnalysisContext,
    dep_info: ErlangDependencyInfo,
    additional_code_path: cmd_args,
    additional_args: [cmd_args, None],
    shell_dep_info: ErlangDependencyInfo,
) -> RunInfo:
    tools = get_toolchain(ctx).otp_binaries
    if len(tools.erl.inputs) != 0:
        erl = cmd_args(cmd_args(tools.erl, delimiter = " "), format = '"${REPO_ROOT}"/{}')
    else:
        erl = cmd_args(cmd_args(tools.erl, delimiter = " "), format = "{}")
    erl_args = cmd_args("exec", erl, delimiter = " \\\n")

    # add paths
    code_path = cmd_args(
        code_path_args(dep_info.code_path_tset),
        additional_code_path,
        code_path_args(shell_dep_info.code_path_tset),
        prepend = "-pa",
        absolute_prefix = '"${REPO_ROOT}"/',
    )
    erl_args.add(code_path)

    # add configs
    config_files = _shell_config_files(ctx)
    erl_args.add(cmd_args(config_files, prepend = "-config", absolute_prefix = '"${REPO_ROOT}"/'))

    # add extra args
    if additional_args:
        erl_args.add(additional_args)
    erl_args.add('"$@"')

    start_shell_content = cmd_args(
        "#!/usr/bin/env bash",
        "export REPO_ROOT=$(buck2 root --kind=project)",
        erl_args,
        "",
    )

    shell_script, _ = ctx.actions.write(
        "start_shell.sh",
        start_shell_content,
        is_executable = True,
        with_inputs = True,
        allow_args = True,
        has_content_based_path = False,
    )

    return RunInfo(shell_script)

def _shell_config_files(ctx: AnalysisContext) -> list[Artifact]:
    config_files = []
    for config_dep in ctx.attrs.shell_configs:
        for artifact in config_dep[DefaultInfo].default_outputs:
            if erlang_build.utils.is_config(artifact):
                config_files.append(artifact)
        for artifact in config_dep[DefaultInfo].other_outputs:
            if erlang_build.utils.is_config(artifact):
                config_files.append(artifact)
    return config_files

erlang_shell = struct(
    build_run_info = _build_run_info,
)
