# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:paths.bzl",
    "paths",
)
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load(":attrs.bzl", "erlang_rule_attributes")
load(
    ":erlang_error_handler.bzl",
    "erlang_action_error_handler",
)
load(
    ":erlang_info.bzl",
    "ErlangOTPBinariesInfo",
    "ErlangParseTransformInfo",
    "ErlangToolchainInfo",
    "ErtsToolchainInfo",
    "Tool",
    "Tools",
)
load(
    ":erlang_paths.bzl",
    "basename_without_extension",
    "strip_extension",
)

Toolchain = ErlangToolchainInfo

ToolchainUtillInfo = provider(
    # @unsorted-dict-items
    fields = {
        "app_src_script": provider_field(Artifact),
        "appup_src_script": provider_field(Artifact),
        "boot_script_builder": provider_field(Artifact),
        "core_parse_transforms": provider_field(list[Dependency]),
        "dependency_analyzer": provider_field(Artifact),
        "dependency_finalizer": provider_field(Artifact),
        "dependency_merger": provider_field(Artifact),
        "escript_trampoline": provider_field(Artifact),
        "escript_builder": provider_field(Artifact),
        "release_variables_builder": provider_field(Artifact),
        "utility_modules": provider_field(list[Artifact]),
    },
)

def get_toolchain(ctx: AnalysisContext) -> Toolchain:
    return ctx.attrs._toolchain[ErlangToolchainInfo]

def _erlang_toolchain_impl(ctx: AnalysisContext) -> list[Provider]:
    """rule for erlang toolchain"""

    erl_opts = ctx.attrs.erl_opts
    emu_flags = ctx.attrs.emu_flags

    # backwards compat: options could be given as a string
    if isinstance(erl_opts, str):
        erl_opts = erl_opts.split()
    if isinstance(emu_flags, str):
        emu_flags = emu_flags.split()

    # get otp binaries
    binaries_info = ctx.attrs.otp_binaries[ErlangOTPBinariesInfo]
    erl = cmd_args(binaries_info.erl, emu_flags)
    erlc = cmd_args(binaries_info.erlc, hidden = binaries_info.erl)
    escript = cmd_args(binaries_info.escript, hidden = binaries_info.erl)
    otp_binaries = Tools(
        erl = erl,
        erlc = erlc,
        escript = escript,
    )

    env = ctx.attrs.env
    if emu_flags and "ERL_FLAGS" not in env:
        env["ERL_FLAGS"] = " ".join(emu_flags)

    # extract utility artefacts
    utils = ctx.attrs.toolchain_utilities[ToolchainUtillInfo]

    core_parse_transforms = _gen_parse_transforms(
        ctx,
        otp_binaries.erlc,
        env,
        utils.core_parse_transforms,
    )

    parse_transforms = _gen_parse_transforms(
        ctx,
        otp_binaries.erlc,
        env,
        ctx.attrs.parse_transforms,
    )
    intersection = [key for key in parse_transforms if key in core_parse_transforms]
    if len(intersection):
        fail("conflicting parse_transform with core parse_transform found: %s" % (repr(intersection),))

    utility_modules = _gen_util_beams(ctx, env, utils.utility_modules, otp_binaries.erlc)

    app_src_script = _gen_toolchain_script(ctx, env, utils.app_src_script, otp_binaries, utility_modules)
    appup_src_script = _gen_toolchain_script(ctx, env, utils.appup_src_script, otp_binaries, utility_modules)
    boot_script_builder = _gen_toolchain_script(ctx, env, utils.boot_script_builder, otp_binaries, utility_modules)
    dependency_analyzer = _gen_toolchain_script(ctx, env, utils.dependency_analyzer, otp_binaries, utility_modules)
    dependency_finalizer = _gen_toolchain_script(ctx, env, utils.dependency_finalizer, otp_binaries, utility_modules)
    dependency_merger = _gen_toolchain_script(ctx, env, utils.dependency_merger, otp_binaries, utility_modules)
    escript_builder = _gen_toolchain_script(ctx, env, utils.escript_builder, otp_binaries, utility_modules)
    release_variables_builder = _gen_toolchain_script(ctx, env, utils.release_variables_builder, otp_binaries, utility_modules)

    erts_toolchain_info = ctx.attrs.erts_toolchain_info[ErtsToolchainInfo] if ctx.attrs.erts_toolchain_info else None

    return [
        DefaultInfo(),
        ErlangToolchainInfo(
            name = ctx.attrs.name,
            app_src_script = app_src_script,
            appup_src_script = appup_src_script,
            boot_script_builder = boot_script_builder,
            dependency_analyzer = dependency_analyzer,
            dependency_finalizer = dependency_finalizer,
            dependency_merger = dependency_merger,
            erl_opts = erl_opts,
            env = ctx.attrs.env,
            emu_flags = emu_flags,
            escript_trampoline = utils.escript_trampoline,
            escript_builder = escript_builder,
            otp_binaries = otp_binaries,
            release_variables_builder = release_variables_builder,
            core_parse_transforms = core_parse_transforms,
            parse_transforms = parse_transforms,
            parse_transforms_filters = ctx.attrs.parse_transforms_filters,
            utility_modules = utility_modules,
            erts_toolchain_info = erts_toolchain_info,
            error_handler = erlang_action_error_handler,
        ),
    ]

def _gen_parse_transforms(ctx: AnalysisContext, erlc: Tool, env: dict[str, str], parse_transforms: list[Dependency]) -> dict[str, cmd_args]:
    transforms = {}
    for dep in parse_transforms:
        src = dep[ErlangParseTransformInfo].source
        extra = dep[ErlangParseTransformInfo].extra_files
        module_name = basename_without_extension(src.basename)
        if module_name in transforms:
            fail("ambiguous global parse_transforms defined: %s", (module_name,))
        (beam, resource_dir) = _gen_parse_transform_beam(ctx, env, src, extra, erlc)
        transform_arg = "+{parse_transform, %s}" % (module_name,)
        if resource_dir:
            transforms[module_name] = cmd_args(transform_arg, cmd_args(beam, format = "-pa{}", parent = 1, hidden = resource_dir))
        else:
            transforms[module_name] = cmd_args(transform_arg, cmd_args(beam, format = "-pa{}", parent = 1))
    return transforms

def _gen_parse_transform_beam(ctx: AnalysisContext, env: dict[str, str], src: Artifact, extra: list[Artifact], erlc: Tool) -> (Artifact, [Artifact, None]):
    name = strip_extension(src.basename)

    # install resources
    resource_dir = None
    if extra:
        resource_dir = ctx.actions.symlinked_dir(
            paths.join(name, "resources"),
            {infile.basename: infile for infile in extra},
            has_content_based_path = False,
        )

    # build beam
    output = ctx.actions.declare_output(name, name + ".beam", has_content_based_path = False)
    _compile_toolchain_module(ctx, env, src, output.as_output(), erlc)

    return output, resource_dir

default_toolchain_script_args_pre = cmd_args(
    "+A0",
    "+S1:1",
    "+sbtu",
    "+SDio",
    "1",
    "+MMscs",
    "8",
    "+MMsco",
    "false",
    "-mode",
    "minimal",
    "-noinput",
    "-noshell",
    "-boot",
    "no_dot_erlang",
    "-eval",
)
default_toolchain_script_args_post = cmd_args("-s", "erlang", "halt", "--")

def _gen_toolchain_script(ctx: AnalysisContext, env: dict[str, str], script: Artifact, tools: Tools, utility_modules: Artifact) -> Tool:
    name = strip_extension(script.basename)
    out = ctx.actions.declare_output(name, name + ".beam", has_content_based_path = False)
    _compile_toolchain_module(ctx, env, script, out.as_output(), tools.erlc)
    eval = cmd_args(name, ":main(init:get_plain_arguments())", delimiter = "")
    return cmd_args(
        tools.erl,
        cmd_args(out, parent = 1, prepend = "-pa"),
        cmd_args(utility_modules, parent = 0, prepend = "-pa"),
        default_toolchain_script_args_pre,
        eval,
        default_toolchain_script_args_post,
    )

erlang_toolchain = rule(
    impl = _erlang_toolchain_impl,
    attrs = {
        "core_parse_transforms": attrs.list(attrs.dep(), default = ["@prelude//erlang/toolchain:transform_project_root"]),
        "emu_flags": attrs.one_of(
            attrs.list(attrs.string()),
            # deprecated: list-of-strings are preferred (e.g. they are more easily selectable)
            attrs.string(),
            default = [],
        ),
        "env": attrs.dict(key = attrs.string(), value = attrs.string(), default = {}),
        "erl_opts": attrs.one_of(
            attrs.list(attrs.string()),
            # deprecated: list-of-strings are preferred (e.g. they are more easily selectable)
            attrs.string(),
            default = [],
        ),
        "erts_toolchain_info": attrs.option(attrs.dep(providers = [ErtsToolchainInfo]), default = None),
        "otp_binaries": attrs.toolchain_dep(),
        "parse_transforms": attrs.list(attrs.dep()),
        "parse_transforms_filters": attrs.dict(key = attrs.string(), value = attrs.list(attrs.string())),
        "toolchain_utilities": attrs.toolchain_dep(default = "@prelude//erlang/toolchain:toolchain_utilities"),
    },
    is_toolchain_rule = True,
)

def _gen_util_beams(ctx: AnalysisContext, env: dict[str, str], sources: list[Artifact], erlc: Tool) -> Artifact:
    beams = []
    for src in sources:
        output = ctx.actions.declare_output(
            paths.join(
                "__build",
                paths.replace_extension(src.basename, ".beam"),
            ),
            has_content_based_path = False,
        )
        _compile_toolchain_module(ctx, env, src, output.as_output(), erlc)
        beams.append(output)

    beam_dir = ctx.actions.symlinked_dir(
        "utility_modules",
        {beam.basename: beam for beam in beams},
        has_content_based_path = False,
    )

    return beam_dir

def _compile_toolchain_module(ctx: AnalysisContext, env: dict[str, str], src: Artifact, out: OutputArtifact, erlc: Tool):
    # NOTE: since we do NOT define +debug_info, this is hermetic
    ctx.actions.run(
        [erlc, "+deterministic", "-o", cmd_args(out, parent = 1), src],
        category = "erlc",
        identifier = src.short_path,
        env = env,
    )

# Parse Transform

def erlang_otp_binaries_impl(ctx: AnalysisContext):
    return [
        DefaultInfo(),
        ErlangOTPBinariesInfo(
            erl = cmd_args(ctx.attrs.erl),
            erlc = cmd_args(ctx.attrs.erlc),
            escript = cmd_args(ctx.attrs.escript),
        ),
    ]

erlang_parse_transform = rule(
    impl = lambda ctx: [
        DefaultInfo(),
        ErlangParseTransformInfo(
            source = ctx.attrs.src,
            extra_files = ctx.attrs.extra_files,
        ),
    ],
    attrs = {
        "extra_files": attrs.list(attrs.source(), default = []),
        "src": attrs.source(),
    },
)

def _toolchain_utils(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        ToolchainUtillInfo(
            app_src_script = ctx.attrs.app_src_script,
            appup_src_script = ctx.attrs.appup_src_script,
            boot_script_builder = ctx.attrs.boot_script_builder,
            core_parse_transforms = ctx.attrs.core_parse_transforms,
            dependency_analyzer = ctx.attrs.dependency_analyzer,
            dependency_finalizer = ctx.attrs.dependency_finalizer,
            dependency_merger = ctx.attrs.dependency_merger,
            escript_trampoline = ctx.attrs.escript_trampoline,
            escript_builder = ctx.attrs.escript_builder,
            release_variables_builder = ctx.attrs.release_variables_builder,
            utility_modules = ctx.attrs.utility_modules,
        ),
    ]

toolchain_utilities = rule(
    impl = _toolchain_utils,
    attrs = {
        "app_src_script": attrs.source(),
        "appup_src_script": attrs.source(),
        "boot_script_builder": attrs.source(),
        "core_parse_transforms": attrs.list(attrs.dep()),
        "dependency_analyzer": attrs.source(),
        "dependency_finalizer": attrs.source(),
        "dependency_merger": attrs.source(),
        "escript_builder": attrs.source(),
        "escript_trampoline": attrs.source(),
        "release_variables_builder": attrs.source(),
        "utility_modules": attrs.list(attrs.source()),
    },
    is_toolchain_rule = True,
)

erlang_otp_binaries = prelude_rule(
    name = "erlang_otp_binaries",
    impl = erlang_otp_binaries_impl,
    docs = """
        This target defines the executables for the Erlang toolchains, and is required to defined a toolchain.
    """,
    examples = """
        erlang_otp_binaries(
            name = "local",
            erl = "local/erl",
            erlc = "local/erlc",
            escript = "local/escript",
        )
    """,
    further = None,
    attrs = erlang_rule_attributes["erlang_otp_binaries"],
    is_toolchain_rule = True,
)
