# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//java:java_toolchain.bzl",
    "JavaToolchainInfo",  # @unused Used as a type
)
load("@prelude//utils:argfile.bzl", "at_argfile")

def _class_to_src_map_args(mapping: Artifact | None):
    if mapping != None:
        return cmd_args(mapping)
    return cmd_args()

JavaClassToSourceMapTset = transitive_set(
    args_projections = {
        "class_to_src_map": _class_to_src_map_args,
    },
)

JavaClassToSourceMapInfo = provider(
    # @unsorted-dict-items
    fields = {
        "tset": provider_field(typing.Any, default = None),
        "debuginfo": provider_field(typing.Any, default = None),
        # Used internally in this module to aid generation of `debuginfo`.
        "_tset_debuginfo": provider_field(typing.Any, default = None),
    },
)

def create_class_to_source_map_info(
    ctx: AnalysisContext, mapping: Artifact | None = None, mapping_debuginfo: Artifact | None = None, deps = [Dependency]
) -> JavaClassToSourceMapInfo:
    # Only generate debuginfo if the debug info tool is available.
    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    tset_debuginfo = None
    debuginfo = None
    if java_toolchain.gen_class_to_source_map_debuginfo != None:
        tset_debuginfo = ctx.actions.tset(
            JavaClassToSourceMapTset,
            value = mapping_debuginfo,
            children = [
                d[JavaClassToSourceMapInfo]._tset_debuginfo
                for d in deps
                if JavaClassToSourceMapInfo in d and d[JavaClassToSourceMapInfo]._tset_debuginfo != None
            ],
        )
        debuginfo = _create_merged_debug_info(
            actions = ctx.actions,
            java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo],
            tset_debuginfo = tset_debuginfo,
            name = ctx.label.name + ".debuginfo_merged.json",
        )

    return JavaClassToSourceMapInfo(
        _tset_debuginfo = tset_debuginfo,
        tset = ctx.actions.tset(
            JavaClassToSourceMapTset,
            value = mapping,
            children = [d[JavaClassToSourceMapInfo].tset for d in deps if JavaClassToSourceMapInfo in d],
        ),
        debuginfo = debuginfo,
    )

def create_class_to_source_map_from_jar(
    actions: AnalysisActions,
    name: str,
    java_toolchain: JavaToolchainInfo,
    jar: Artifact,
    srcs: list[Artifact],
    owner_target: str,
    sources_jar_name: [str, None] = None,
    debuginfo: Artifact | None = None,
    compiler_map: Artifact | None = None,
) -> (Artifact, Artifact | None):
    output = actions.declare_output(name, has_content_based_path = True)
    tool = java_toolchain.compiler_class_to_source_map if compiler_map != None else java_toolchain.gen_class_to_source_map
    cmd = cmd_args(tool[RunInfo])
    if java_toolchain.gen_class_to_source_map_include_sourceless_compiled_packages != None:
        for item in java_toolchain.gen_class_to_source_map_include_sourceless_compiled_packages:
            cmd.add("-i", item)
    cmd.add("-o", output.as_output())
    if compiler_map != None:
        cmd.add("--compiler-map", compiler_map)
    elif debuginfo != None:
        cmd.add("--debuginfo", debuginfo)
    cmd.add("--owner-target", owner_target)
    cmd.add(jar)
    cmd.add(at_argfile(actions = actions, name = "class_to_srcs_map_argsfile.txt", args = srcs, has_content_based_path = True))
    sources_jar = None
    if sources_jar_name:
        sources_jar = actions.declare_output(sources_jar_name, has_content_based_path = True)
        cmd.add("--sources_jar", sources_jar.as_output())
    actions.run(cmd, category = "class_to_srcs_map_from_compiler" if compiler_map != None else "class_to_srcs_map")
    return (output, sources_jar)

def maybe_create_class_to_source_map_debuginfo(actions: AnalysisActions, name: str, java_toolchain: JavaToolchainInfo, srcs: list[Artifact]) -> Artifact | None:
    # Only generate debuginfo if the debug info tool is available.
    if java_toolchain.gen_class_to_source_map_debuginfo == None:
        return None

    output = actions.declare_output(name, has_content_based_path = True)
    cmd = cmd_args(java_toolchain.gen_class_to_source_map_debuginfo[RunInfo])
    cmd.add("gen")
    cmd.add("-o", output.as_output())
    cmd.add(at_argfile(actions = actions, name = "sourcefiles.txt", args = srcs, has_content_based_path = True))
    actions.run(cmd, category = "class_to_srcs_map_debuginfo")
    return output

def merge_class_to_source_map_from_jar(
    actions: AnalysisActions, name: str, java_toolchain: JavaToolchainInfo, relative_to: [CellRoot, None], deps: list[JavaClassToSourceMapInfo]
) -> Artifact:
    output = actions.declare_output(name, has_content_based_path = True)

    tset = actions.tset(
        JavaClassToSourceMapTset,
        value = None,
        children = [d.tset for d in deps],
    )
    class_to_source_files = tset.project_as_args("class_to_src_map")
    mappings_file = actions.write("class_to_src_map.txt", class_to_source_files, has_content_based_path = True)

    cmd = cmd_args(
        java_toolchain.merge_class_to_source_maps[RunInfo],
        cmd_args(output.as_output(), format = "--output={}"),
        cmd_args(str(relative_to), format = "--relative-to={}") if relative_to != None else [],
        ["--mappings", mappings_file],
        hidden = class_to_source_files,
    )
    actions.run(cmd, category = "merge_class_to_srcs_map")
    return output

def _create_merged_debug_info(actions: AnalysisActions, java_toolchain: JavaToolchainInfo, tset_debuginfo: TransitiveSet, name: str):
    output = actions.declare_output(name, has_content_based_path = True)
    cmd = cmd_args(java_toolchain.gen_class_to_source_map_debuginfo[RunInfo])
    cmd.add("merge")
    cmd.add(cmd_args(output.as_output(), format = "-o={}"))

    tset = actions.tset(
        JavaClassToSourceMapTset,
        children = [tset_debuginfo],
    )
    input_files = tset.project_as_args("class_to_src_map")
    cmd.add(at_argfile(actions = actions, name = "debuginfo_list.txt", args = input_files, has_content_based_path = True))

    actions.run(cmd, category = "merged_debuginfo")
    return output
