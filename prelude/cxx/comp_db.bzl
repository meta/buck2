# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxInternalTools", "CxxPlatformInfo", "CxxToolchainInfo")
load("@prelude//utils:argfile.bzl", "at_argfile")
load(
    ":compile_types.bzl",
    "CxxSrcCompileCommand",  # @unused Used as a type
    "CxxSrcPrecompileCommand",  # @unused Used as a type
)
load(":cxx_context.bzl", "get_cxx_toolchain_info")
load(":cxx_library_utility.bzl", "EMPTY_DEFAULT_INFO")

# Provider that exposes the compilation database information
CxxCompilationDbInfo = provider(
    fields = {
        "info": provider_field(typing.Any, default = None),  # A map of the file (an `Artifact`) to its corresponding `CxxSrcCompileCommand`
        "platform": provider_field(typing.Any, default = None),  # platform for this compilation database
        "precompile_info": provider_field(typing.Any, default = {}),  # A map of the file (an `Artifact`) to its corresponding `CxxSrcPrecompileCommand`
        "toolchain": provider_field(typing.Any, default = None),  # toolchain for this compilation database
    }
)

def make_compilation_db_info(
    src_compile_cmds: list[CxxSrcCompileCommand],
    precompile_cmds: list[CxxSrcPrecompileCommand],
    toolchain_info: CxxToolchainInfo,
    platform_info: CxxPlatformInfo,
) -> CxxCompilationDbInfo:
    return CxxCompilationDbInfo(
        info = {compile_cmd.src: compile_cmd for compile_cmd in src_compile_cmds},
        precompile_info = {compile_cmd.src: compile_cmd for compile_cmd in precompile_cmds},
        platform = platform_info,
        toolchain = toolchain_info,
    )

def _comp_database_entry_path(identifier: str, src_path: str) -> str:
    return paths.join(identifier, "__comp_db__", src_path + ".comp_db.json")

def _create_comp_database_impl(
    actions: AnalysisActions,
    internal_tools: CxxInternalTools,
    identifier: str,
    src_compile_cmds: list[CxxSrcCompileCommand],
    db: OutputArtifact,
    entries: dict[str, OutputArtifact],
) -> list[Provider]:
    mk_comp_db = internal_tools.make_comp_db

    # Generate the per-source compilation DB entries.
    entries_as_input = []
    for src_compile_cmd in src_compile_cmds:
        cdb_path = _comp_database_entry_path(identifier, src_compile_cmd.src.short_path)
        entry = entries.pop(cdb_path, None)
        if entry:
            cmd = cmd_args(
                mk_comp_db,
                "gen",
                cmd_args(entry, format = "--output={}"),
                src_compile_cmd.src.basename,
                cmd_args(src_compile_cmd.src, parent = 1),
                "--",
                src_compile_cmd.cxx_compile_cmd.base_compile_cmd,
                src_compile_cmd.cxx_compile_cmd.argsfile.cmd_form,
                src_compile_cmd.args,
            )
            entry_identifier = paths.join(identifier, src_compile_cmd.src.short_path)
            actions.run(cmd, category = "cxx_compilation_database", identifier = entry_identifier)
            entries_as_input.append(entry.as_input())

    # Merge all entries into the actual compilation DB.
    cmd = cmd_args(mk_comp_db)
    cmd.add("merge")
    cmd.add(cmd_args(db, format = "--output={}"))

    cmd.add(
        at_argfile(
            actions = actions,
            name = identifier + ".cxx_comp_db_argsfile",
            args = entries_as_input,
        )
    )

    actions.run(cmd, category = "cxx_compilation_database_merge", identifier = identifier)

    return [EMPTY_DEFAULT_INFO]

_dynamic_compilation_database_rule = dynamic_actions(
    impl = _create_comp_database_impl,
    attrs = {
        "db": dynattrs.output(),
        "entries": dynattrs.dict(str, dynattrs.output()),
        "identifier": dynattrs.value(str),
        "internal_tools": dynattrs.value(CxxInternalTools),
        "src_compile_cmds": dynattrs.list(dynattrs.value(CxxSrcCompileCommand)),
    },
)

def create_compilation_database(ctx: AnalysisContext, src_compile_cmds: list[CxxSrcCompileCommand], identifier: str) -> DefaultInfo:
    actions = ctx.actions

    # Generate the per-source compilation DB entries.
    entries = {}
    other_outputs = []

    for src_compile_cmd in src_compile_cmds:
        cdb_path = _comp_database_entry_path(identifier, src_compile_cmd.src.short_path)
        if cdb_path not in entries:
            entry = actions.declare_output(cdb_path, has_content_based_path = False)

            # Add all inputs the command uses to runtime files.
            other_outputs.append(entry)
            other_outputs.append(src_compile_cmd.src)
            other_outputs.append(src_compile_cmd.cxx_compile_cmd.base_compile_cmd)
            other_outputs.append(src_compile_cmd.cxx_compile_cmd.argsfile.cmd_form)
            entries[cdb_path] = entry.as_output()

    # Merge all entries into the actual compilation DB.
    db = actions.declare_output(paths.join(identifier, "compile_commands.json"), has_content_based_path = False)

    # We don't have any dynamic inputs here and use dynamic action purely as an optimization
    # so we don't occupy memory in analysis graph until we actually need compilation database.
    actions.dynamic_output_new(
        _dynamic_compilation_database_rule(
            db = db.as_output(),
            entries = entries,
            identifier = identifier,
            internal_tools = get_cxx_toolchain_info(ctx).internal_tools,
            src_compile_cmds = src_compile_cmds,
        ),
    )

    return DefaultInfo(default_output = db, other_outputs = other_outputs)
