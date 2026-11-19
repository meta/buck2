# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load(":attrs.bzl", "erlang_rule_attributes")
load(":erlang_build.bzl", "erlang_build")
load(":erlang_dependencies.bzl", "flatten_dependencies")
load(":erlang_info.bzl", "ErlangAppInfo")
load(":erlang_paths.bzl", "has_extension")
load(
    ":erlang_toolchain.bzl",
    "Toolchain",  # @unused Used as type
    "get_toolchain",
)

def erlang_escript_impl(ctx: AnalysisContext) -> list[Provider]:
    toolchain = get_toolchain(ctx)

    # collect all dependencies
    dependencies = flatten_dependencies(ctx.attrs.deps)
    artifacts = {}

    for dep in dependencies.values():
        if ErlangAppInfo not in dep:
            # skip extra includes
            continue
        dep_info = dep[ErlangAppInfo]
        if dep_info.virtual:
            # skip virtual apps
            continue
        app_folder = dep_info.app_folder

        artifacts[_ebin_path(dep_info.name)] = app_folder.project("ebin")
        if ctx.attrs.include_priv:
            artifacts[_priv_path(dep_info.name)] = app_folder.project("priv")

    # additional resources
    for res in ctx.attrs.resources:
        for artifact in res[DefaultInfo].default_outputs:
            if artifact.short_path in artifacts:
                fail("multiple artifacts defined for path %s", (artifact.short_path))
            artifacts[artifact.short_path] = artifact
        for artifact in res[DefaultInfo].other_outputs:
            if artifact.short_path in artifacts:
                fail("multiple artifacts defined for path %s", (artifact.short_path))
            artifacts[artifact.short_path] = artifact

    escript_name = _escript_name(ctx)
    output = ctx.actions.declare_output(escript_name, has_content_based_path = False)

    args = ctx.attrs.emu_args

    config_files = _escript_config_files(ctx)
    for config_file in config_files:
        artifacts[config_file.short_path] = config_file

    escript_trampoline = build_escript_bundled_trampoline(ctx, toolchain, config_files)
    artifacts[escript_trampoline.basename] = escript_trampoline

    args += ["-escript", "main", "erlang_escript_trampoline"]

    escript_build_spec = {
        "artifacts": artifacts,
        "emu_args": args,
        "output": output.as_output(),
    }

    spec_file = ctx.actions.write_json(
        "escript_build_spec.json",
        escript_build_spec,
        with_inputs = True,
        has_content_based_path = False,
    )

    create_escript(ctx, spec_file, toolchain, escript_name)

    escript_cmd = cmd_args(
        [
            toolchain.otp_binaries.escript,
            output,
        ],
    )

    return [
        DefaultInfo(default_output = output),
        RunInfo(escript_cmd),
    ]

def create_escript(ctx: AnalysisContext, spec_file: WriteJsonCliArgs, toolchain: Toolchain, escript_name: str) -> None:
    """build the escript with the escript builder tool"""

    erlang_build.utils.run_with_env(
        ctx,
        toolchain,
        cmd_args(toolchain.escript_builder, spec_file),
        category = "escript",
        identifier = escript_name,
    )
    return None

def _escript_name(ctx: AnalysisContext) -> str:
    if ctx.attrs.script_name:
        return ctx.attrs.script_name
    else:
        return ctx.attrs.name + ".escript"

def _main_module(ctx: AnalysisContext) -> str:
    if ctx.attrs.main_module:
        return ctx.attrs.main_module
    else:
        return ctx.attrs.name

def build_escript_unbundled_trampoline(ctx: AnalysisContext, config_files: list[Artifact]) -> Artifact:
    data = cmd_args()

    data.add("#!/usr/bin/env escript")
    data.add("%% -*- erlang -*-")
    data.add("%%! {}".format(" ".join(ctx.attrs.emu_args)))

    data.add("-module('{}').".format(_escript_name(ctx)))
    data.add("-export([main/1]).")
    data.add("main(Args) ->")
    data.add("EscriptDir = filename:dirname(escript:script_name()),")
    data.add(_config_files_code_to_erl(config_files))
    data.add('    EBinDirs = filelib:wildcard(filename:join([EscriptDir, "lib", "*", "ebin"])),')
    data.add("    code:add_paths(EBinDirs),")
    data.add("    {}:main(Args).".format(_main_module(ctx)))
    data.add(_parse_bin())

    return ctx.actions.write(
        paths.join(erlang_build.utils.BUILD_DIR, "run.escript"),
        data,
        is_executable = True,
        has_content_based_path = False,
    )

def build_escript_bundled_trampoline(ctx: AnalysisContext, toolchain, config_files: list[Artifact]) -> Artifact:
    data = cmd_args()

    data.add(
        """-module('erlang_escript_trampoline').
-export([main/1]).
main(Args) ->
EscriptDir = escript:script_name(),""",
    )
    data.add(_config_files_code_to_erl(config_files))
    data.add("    {}:main(Args).".format(_main_module(ctx)))
    data.add(_parse_bin())
    escript_trampoline_erl = ctx.actions.write(
        paths.join(erlang_build.utils.BUILD_DIR, "erlang_escript_trampoline.erl"),
        data,
        has_content_based_path = False,
    )
    my_output = ctx.actions.declare_output("erlang_escript_trampoline.beam", has_content_based_path = False)

    erlang_build.utils.run_with_env(
        ctx,
        toolchain,
        cmd_args(
            toolchain.otp_binaries.erlc,
            "-o",
            cmd_args(my_output.as_output(), parent = 1),
            escript_trampoline_erl,
        ),
        category = "erlc_escript_trampoline",
    )

    return my_output

def _ebin_path(app_name: str) -> str:
    return paths.join(app_name, "ebin")

def _priv_path(app_name: str) -> str:
    return paths.join(app_name, "priv")

def _escript_config_files(ctx: AnalysisContext) -> list[Artifact]:
    config_files = []
    for config_dep in ctx.attrs.configs:
        for artifact in config_dep[DefaultInfo].default_outputs:
            if has_extension(artifact.short_path, ".config"):
                config_files.append(artifact)
        for artifact in config_dep[DefaultInfo].other_outputs:
            if has_extension(artifact.short_path, ".config"):
                config_files.append(artifact)
    return config_files

def _config_files_code_to_erl(config_files: list[Artifact]) -> list[str]:
    cmd = []
    cmd.append("ConfigFiles = [")
    for i in range(0, len(config_files)):
        cmd.append('"{}"'.format(config_files[i].short_path))
        if i < len(config_files) - 1:
            cmd.append(",")
    cmd.append(
        """],
[begin
{ok, AppConfigBin, _FullName} = erl_prim_loader:get_file(filename:join(EscriptDir, ConfigFile)),
{ok, AppConfig} = parse_bin(AppConfigBin),
ok = application:set_env(AppConfig, [{persistent, true}])
end || ConfigFile <- ConfigFiles],""",
    )
    return cmd

def _parse_bin() -> str:
    return """
parse_bin(<<"">>) ->
    [];
parse_bin(Bin) ->
        {ok, Tokens, _} = erl_scan:string(binary_to_list(Bin)),
        erl_parse:parse_term(Tokens).
        """

erlang_escript = prelude_rule(
    name = "erlang_escript",
    impl = erlang_escript_impl,
    docs = """
        The `erlang_escript` target builds and runs bundled escripts. Please refer to the
        [OTP documentation](https://www.erlang.org/doc/man/escript.html) for more details about escripts.

        Escripts by default always try to use the module that has the same name as the escripts basename as entry point, e.g. if
        the escript is called `script.escript` then running the escript will try to call `script:main/1`. Both name and
        main module can be overwritten though.

        The target name doubles as the default escript name. If the `main_module` attribute is not used, the escript filename will
        be `<name>.escript`.
    """,
    examples = """
        ```
        erlang_escript(
            name = "script",
            main_module = "main_module",
            script_name = "the_script",
            deps = [
                ":escript_app",
            ],
            emu_args = ["+sbtu", "+A1"],
        )

        erlang_application(
            name = "escript_app",
            srcs = ["src/main_module.erl"],
            applications = [
                "kernel",
                "stdlib",
            ],
        )
        ```
    """,
    further = None,
    attrs = erlang_rule_attributes["erlang_escript"],
)
