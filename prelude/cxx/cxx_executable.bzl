# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactTSet",
    "make_artifact_tset",
    "project_artifacts",
)
load("@prelude//:attrs_validators.bzl", "get_attrs_validation_specs")
load("@prelude//:local_only.bzl", "get_resolved_cxx_binary_link_execution_preference")
load(
    "@prelude//:resources.bzl",
    "create_relocatable_resources_info",
    "create_resource_db",
    "gather_resources",
)
load(
    "@prelude//apple:apple_frameworks.bzl",
    "apple_build_link_args_with_deduped_flags",
    "apple_create_frameworks_linkable",
    "apple_get_link_info_by_deduping_link_infos",
    "get_framework_search_path_flags",
)
load(
    "@prelude//cxx:cxx_bolt.bzl",
    "PRE_BOLT_SUFFIX",
    "cxx_use_bolt",
)
load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "IncrementalLinkingMode",
    "PicBehavior",
    "RuntimeDependencyHandling",
)
load(
    "@prelude//cxx:hip_debug_extract.bzl",
    "PRE_EXTRACT_SUFFIX",
    "hip_debug_extract_available",
)
load(
    "@prelude//cxx:link_groups_types.bzl",
    "LinkGroupsDebugLinkInfo",
    "LinkGroupsDebugLinkableItem",
)
load("@prelude//cxx:transformation_spec.bzl", "TransformationKind", "build_transformation_spec_context")
load(
    "@prelude//dist:dist_info.bzl",
    "DistInfo",
)
load(
    "@prelude//ide_integrations/xcode:argsfiles.bzl",
    "XCODE_ARGSFILES_SUB_TARGET",
)
load(
    "@prelude//ide_integrations/xcode:data.bzl",
    "XCODE_DATA_SUB_TARGET",
    "XcodeDataInfo",
    "generate_xcode_data",
)
load("@prelude//linking:add_elf_sections.bzl", "PRE_ADD_ELF_SECTIONS_SUFFIX", "get_elf_sections")
load("@prelude//linking:execution_preference.bzl", "LinkExecutionPreference")
load(
    "@prelude//linking:generated_build_info.bzl",
    "compile_generated_build_info",
    "generate_build_info",
    "generate_build_info_shared_library",
    "generated_build_info_is_shared_library",
)
load(
    "@prelude//linking:link_groups.bzl",
    "gather_link_group_libs",
)
load(
    "@prelude//linking:link_info.bzl",
    "LibOutputStyle",
    "LinkArgs",
    "LinkCommandDebugOutput",
    "LinkInfo",
    "LinkOrdering",
    "LinkStrategy",
    "LinkedObject",
    "ObjectsLinkable",
    "get_lib_output_style",
    "make_link_command_debug_output",
    "make_link_command_debug_output_json_info",
    "process_link_strategy_for_pic_behavior",
    "to_link_strategy",
    "unpack_link_args",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "LinkableGraph",
    "create_linkable_graph",
    "get_linkable_graph_node_map_func",
    "reduce_linkable_graph",
)
load(
    "@prelude//linking:linkables.bzl",
    "LinkableProviders",
    "linkables",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibrary",
    "SharedLibraryInfo",
    "merge_shared_libraries",
    "traverse_shared_library_info",
)
load("@prelude//linking:stamp_build_info.bzl", "PRE_STAMPED_SUFFIX", "cxx_stamp_build_info")
load("@prelude//linking:types.bzl", "LinkPreference")
load("@prelude//utils:arglike.bzl", "ArgLike")
load(
    "@prelude//utils:build_graph_pattern.bzl",
    "new_build_graph_info",
)
load("@prelude//utils:lazy.bzl", "lazy")
load(
    "@prelude//utils:utils.bzl",
    "flatten",
    "flatten_dict",
    "map_val",
)
load("@prelude//xplugins:debug_artifacts.bzl", "xplugins_get_debug_artifacts_info")
load(
    "@prelude//xplugins:types.bzl",
    "XPluginsDebugArtifactsInfo",
)
load(
    ":argsfiles.bzl",
    "ARGSFILES_SUBTARGET",
    "get_argsfiles_output",
)
load(
    ":comp_db.bzl",
    "CxxCompilationDbInfo",
    "create_compilation_database",
    "make_compilation_db_info",
)
load(
    ":compile.bzl",
    "ClangTracesInfo",
    "ClangTracesTSet",
    "PicClangTracesInfo",
    "compile_cxx",
    "create_compile_cmds",
    "cxx_objects_sub_targets",
)
load(":compile_types.bzl", "CxxCompileFlavor")
load(":cxx_context.bzl", "get_cxx_platform_info", "get_cxx_toolchain_info")
load(
    ":cxx_instrumentation.bzl",
    "is_coverage_enabled_by_any_dep",
)
load(
    ":cxx_library_utility.bzl",
    "OBJECTS_SUBTARGET",
    "cxx_attr_deps",
    "cxx_attr_link_style",
    "cxx_attr_linker_flags",
    "cxx_attr_resources",
    "cxx_is_gnu",
)
load(
    ":cxx_link_utility.bzl",
    "executable_shared_lib_arguments",
)
load(":cxx_types.bzl", "CxxRuleConstructorParams")
load(":diagnostics.bzl", "check_sub_target")
load(":gcno.bzl", "GcnoFilesInfo")
load(":groups.bzl", "get_dedupped_roots_from_groups")
load(
    ":link.bzl",
    "CxxGcSectionsData",
    "CxxLinkerMapData",
    "collect_hip_debug_from_links",
    "cxx_link_into",
    "link_args_have_hip_device_debug",
)
load(
    ":link_groups.bzl",
    "BuildLinkGroupsContext",
    "FinalLabelsToLinks",
    "LINK_GROUPS_INFO_SUB_TARGET",
    "LINK_GROUP_MAPPINGS_FILENAME_SUFFIX",
    "LINK_GROUP_MAPPINGS_SUB_TARGET",
    "LINK_GROUP_MAP_DATABASE_SUB_TARGET",
    "LinkGroupContext",
    "build_shared_libs_for_symlink_tree",
    "collect_linkables",
    "create_debug_linkable_entries",
    "create_link_groups",
    "find_relevant_roots",
    "get_filtered_labels_to_links_map",
    "get_filtered_links",
    "get_filtered_targets",
    "get_link_group",
    "get_link_group_map_json",
    "get_link_group_preferred_linkage",
    "get_public_link_group_nodes",
    "get_transitive_deps_matching_labels",
)
load(
    ":link_types.bzl",
    "CxxLinkResultType",
    "LinkOptions",
    "link_options",
    "merge_link_options",
)
load(
    ":linker.bzl",
    "DUMPBIN_SUB_TARGET",
    "PDB_SUB_TARGET",
    "get_dumpbin_providers",
    "get_pdb_providers",
    "get_shared_library_name",
)
load(
    ":preprocessor.bzl",
    "CPreprocessor",
    "CPreprocessorArgs",
    "cxx_inherited_preprocessor_infos",
    "cxx_private_preprocessor_info",
)

CxxExecutableOutput = record(
    binary = Artifact,
    unstripped_binary = Artifact,
    bitcode_bundle = field(Artifact | None, None),
    diagnostics = field(Artifact | None),
    dwp = field(Artifact | None),
    # Files that must be present for the executable to run successfully. These
    # are always materialized, whether the executable is the output of a build
    # or executed as a host tool. They become hidden= arguments when executing
    # the executable via RunInfo().
    runtime_files = list[ArgLike],
    sub_targets = dict[str, list[DefaultInfo]],
    # The LinkArgs used to create the final executable in 'binary'.
    link_args = list[LinkArgs],
    # External components needed to debug the executable.
    external_debug_info = field(ArtifactTSet, ArtifactTSet()),
    # The projection of `external_debug_info`. These files need to be
    # materialized when this executable is the output of a build, not when it is
    # used by other rules. They become other_outputs on DefaultInfo.
    external_debug_info_artifacts = list[TransitiveSetArgsProjection],
    shared_libs = list[SharedLibrary],
    # All link group links that were generated in the executable.
    auto_link_groups = field(dict[str, LinkedObject], {}),
    compilation_db = CxxCompilationDbInfo,
    xcode_data = [XcodeDataInfo, None],
    linker_map_data = [CxxLinkerMapData, None],
    gc_sections_data = [CxxGcSectionsData, None],
    link_command_debug_output = field([LinkCommandDebugOutput, None], None),
    dist_info = DistInfo,
    sanitizer_runtime_files = field(list[Artifact], []),
    index_stores = field(list[Artifact], []),
    validation_specs = field(list[ValidationSpec], []),
    gcno_files = field(list[Artifact], []),
    xplugins_debug_artifacts_info = field(XPluginsDebugArtifactsInfo | None, None),
    build_info_manifest_entries = field(Artifact | None, None),
)

def cxx_executable(ctx: AnalysisContext, impl_params: CxxRuleConstructorParams, is_cxx_test: bool = False) -> CxxExecutableOutput:
    # Gather preprocessor inputs.
    preprocessor_deps = cxx_attr_deps(ctx) + filter(None, [impl_params.precompiled_header])
    (own_preprocessor_info, test_preprocessor_infos) = cxx_private_preprocessor_info(
        ctx,
        impl_params.headers_layout,
        raw_headers = ctx.attrs.raw_headers,
        extra_preprocessors = impl_params.extra_preprocessors,
        non_exported_deps = preprocessor_deps,
        is_test = is_cxx_test,
    )
    inherited_preprocessor_infos = cxx_inherited_preprocessor_infos(preprocessor_deps) + impl_params.extra_preprocessors_info

    # Add framework search paths if frameworks attribute is set.
    # This is needed for the apple_test/apple_binary -> cxx_test/cxx_binary swap.
    frameworks = getattr(ctx.attrs, "frameworks", [])
    framework_search_path_pre = None
    if frameworks:
        framework_search_paths_flags = get_framework_search_path_flags(ctx)
        framework_search_path_pre = CPreprocessor(
            args = CPreprocessorArgs(args = [framework_search_paths_flags]),
        )

    # The link style to use.
    link_strategy = to_link_strategy(cxx_attr_link_style(ctx))
    link_strategy = process_link_strategy_for_pic_behavior(link_strategy, get_cxx_toolchain_info(ctx).pic_behavior)

    sub_targets = {}

    # Compile objects.
    own_preprocessors = [own_preprocessor_info] + test_preprocessor_infos
    if framework_search_path_pre:
        own_preprocessors.insert(0, framework_search_path_pre)
    compile_cmd_output = create_compile_cmds(
        ctx.actions,
        ctx.label,
        get_cxx_toolchain_info(ctx),
        impl_params,
        own_preprocessors,
        inherited_preprocessor_infos,
        is_coverage_enabled_by_any_dep(ctx, preprocessor_deps),
    )
    cxx_deps = cxx_attr_deps(ctx)
    build_graph_info = new_build_graph_info(ctx, cxx_deps)
    transformation_spec_context = build_transformation_spec_context(ctx, build_graph_info)
    compile_flavors = set([CxxCompileFlavor("pic")]) if link_strategy != LinkStrategy("static") else set()

    if transformation_spec_context:
        transformation_kind = transformation_spec_context.provider.determine_transformation(ctx.label, transformation_spec_context.graph_info)
        if transformation_kind == TransformationKind("debug"):
            compile_flavors.add(CxxCompileFlavor("debug"))
        elif transformation_kind == TransformationKind("optimized"):
            compile_flavors.add(CxxCompileFlavor("optimized"))

    cxx_outs = compile_cxx(
        actions = ctx.actions,
        target_label = ctx.label,
        toolchain = get_cxx_toolchain_info(ctx),
        src_compile_cmds = compile_cmd_output.src_compile_cmds,
        flavors = compile_flavors,
        provide_syntax_only = True,
        separate_debug_info = impl_params.separate_debug_info,
        use_header_units = impl_params.use_header_units,
        cuda_compile_style = impl_params.cuda_compile_style,
    )

    gcno_files = [out.gcno_file for out in cxx_outs if out.gcno_file]
    if get_cxx_toolchain_info(ctx).gcno_files:
        gcno_files += flatten([dep[GcnoFilesInfo].gcno_files for dep in cxx_deps if GcnoFilesInfo in dep])

    if impl_params.generate_sub_targets.argsfiles:
        sub_targets[ARGSFILES_SUBTARGET] = [get_argsfiles_output(ctx, compile_cmd_output.argsfiles.relative, ARGSFILES_SUBTARGET)]
    if impl_params.generate_sub_targets.xcode_data:
        sub_targets[XCODE_ARGSFILES_SUB_TARGET] = [get_argsfiles_output(ctx, compile_cmd_output.argsfiles.xcode, XCODE_ARGSFILES_SUB_TARGET)]
    sub_targets[OBJECTS_SUBTARGET] = [DefaultInfo(sub_targets = cxx_objects_sub_targets(cxx_outs))]

    if impl_params.generate_sub_targets and impl_params.generate_sub_targets.clang_traces:
        traces = [out.clang_trace for out in cxx_outs if out.clang_trace != None]
        sub_targets["clang-trace"] = [
            DefaultInfo(
                default_outputs = traces,
            )
        ]

        clang_traces_tset = ctx.actions.tset(
            ClangTracesTSet,
            value = traces,
            children = [
                info.clang_traces
                for info in filter(None, [x.get(PicClangTracesInfo) if link_strategy != LinkStrategy("static") else x.get(ClangTracesInfo) for x in cxx_deps])
            ],
        )

        clang_traces_args = clang_traces_tset.project_as_args("clang_traces")
        all_traces = ctx.actions.write(
            ctx.actions.declare_output("recursive_clang_traces.txt", has_content_based_path = False),
            clang_traces_args,
        )

        sub_targets["clang-traces"] = [
            DefaultInfo(
                default_output = all_traces,
                other_outputs = [clang_traces_args],
            )
        ]

    diagnostics = {
        compile_cmd.src.short_path: out.diagnostics for compile_cmd, out in zip(compile_cmd_output.src_compile_cmds, cxx_outs) if out.diagnostics != None
    }
    clang_tidy_diagnostics = {
        compile_cmd.src.short_path + ".clang-tidy": out.clang_tidy_diagnostics
        for compile_cmd, out in zip(compile_cmd_output.src_compile_cmds, cxx_outs)
        if out.clang_tidy_diagnostics != None
    }
    all_diagnostics = None
    if len(diagnostics) > 0:
        sub_targets["check"], all_diagnostics = check_sub_target(
            ctx,
            diagnostics,
            error_handler = impl_params.error_handler,
            extra_sub_targets = clang_tidy_diagnostics,
        )
    if len(clang_tidy_diagnostics) > 0:
        check_all_diagnostics = dict(diagnostics)
        check_all_diagnostics.update(clang_tidy_diagnostics)
        check_all_subtarget_result = check_sub_target(
            ctx,
            check_all_diagnostics,
            error_handler = impl_params.error_handler,
            output_name = "all_diagnostics.txt",
        )
        sub_targets["check-all"] = check_all_subtarget_result[0]

    # Compilation DB.
    comp_db = create_compilation_database(ctx, compile_cmd_output.src_compile_cmds, "compilation-database")
    sub_targets["compilation-database"] = [comp_db]

    # Compilation DB including headers.
    comp_db = create_compilation_database(ctx, compile_cmd_output.comp_db_compile_cmds, "full-compilation-database")
    sub_targets["full-compilation-database"] = [comp_db]

    # comp_db_compile_cmds can include header files being compiled as C++ which should not be exposed in the [compilation-database] subtarget
    comp_db_info = make_compilation_db_info(
        src_compile_cmds = compile_cmd_output.comp_db_compile_cmds,
        precompile_cmds = [],
        toolchain_info = get_cxx_toolchain_info(ctx),
        platform_info = get_cxx_platform_info(ctx),
    )

    # Index Stores created by cxx compile
    index_stores = [out.index_store for out in cxx_outs if out.index_store]

    # Link deps
    link_deps = linkables(cxx_deps) + impl_params.extra_link_deps

    # Link Groups
    link_group = get_link_group(ctx)
    link_group_info = impl_params.link_group_info

    if link_group_info:
        link_groups = link_group_info.groups
        link_group_mappings = link_group_info.mappings
        link_group_deps = [link_group_info.graph]
    else:
        link_groups = {}
        link_group_mappings = {}
        link_group_deps = []
    link_group_preferred_linkage = get_link_group_preferred_linkage(link_groups.values())
    link_groups_enabled = link_group_info != None

    # Create the linkable graph with the binary's deps and any link group deps.
    linkable_graph = create_linkable_graph(
        ctx,
        deps = filter(
            None,
            # Some subtargets (like :Foo[headers]) do not have a linkable_graph.
            [d.linkable_graph for d in link_deps]
            +
            # We also need to include additional link roots, so that we find
            # deps that might need to be linked into the main executable.
            [d.linkable_graph for d in impl_params.extra_link_roots]
            +
            # For non-auto-link-group cases, also search the targets specified
            # in the link group mappings, as they won't be available in other
            # ways.
            link_group_deps,
        ),
    )

    # Gather link inputs.
    own_link_flags = (
        get_cxx_toolchain_info(ctx).linker_info.binary_linker_flags
        + cxx_attr_linker_flags(ctx, impl_params.cxx_flags)
        + impl_params.extra_link_flags
        + impl_params.extra_exported_link_flags
    )

    own_exe_link_flags = impl_params.extra_binary_link_flags + own_link_flags
    deps_merged_link_infos = [d.merged_link_info for d in link_deps]
    frameworks_linkable = apple_create_frameworks_linkable(ctx)
    swiftmodule_linkable = impl_params.swiftmodule_linkable

    # Link group libs.
    link_group_libs = {}

    # Target label to which link group it was included
    targets_consumed_by_link_groups = {}
    auto_link_groups = {}

    # Linker map data for link group shared libraries, keyed by group name.
    link_group_linker_map_data = {}
    labels_to_links = FinalLabelsToLinks(
        map = {},
    )

    if not link_group_mappings:
        # We cannot support deriving link execution preference off the included links, as we've already
        # lost the information on what is in the link.
        # TODO(T152860998): Derive link_execution_preference based upon the included links
        link_execution_preference = (
            LinkExecutionPreference("remote")
            if impl_params.force_remote_link_if_capable
            else get_resolved_cxx_binary_link_execution_preference(ctx, [], impl_params.force_full_hybrid_if_capable)
        )

        dep_links = apple_build_link_args_with_deduped_flags(
            ctx,
            deps_merged_link_infos,
            frameworks_linkable,
            link_strategy,
            transformation_spec_context,
            swiftmodule_linkable,
            prefer_stripped = impl_params.prefer_stripped_objects,
        )
    else:
        reduced_linkable_graph = reduce_linkable_graph(linkable_graph)
        linkable_graph_node_map = reduced_linkable_graph.nodes

        # Although these aren't really deps, we need to search from the
        # extra link group roots to make sure we find additional libs
        # that should be linked into the main link group.
        link_group_extra_link_roots = [d.linkable_graph.nodes.value.label for d in impl_params.extra_link_roots if d.linkable_graph != None]

        exec_dep_roots = [d.linkable_graph.nodes.value.label for d in link_deps if d.linkable_graph != None]

        # If we're using auto-link-groups, where we generate the link group links
        # in the prelude, the link group map will give us the link group libs.
        # Otherwise, pull them from the `LinkGroupLibInfo` provider from out deps.

        public_link_group_nodes = get_public_link_group_nodes(
            linkable_graph_node_map,
            link_group_mappings,
            exec_dep_roots + link_group_extra_link_roots,
            link_group,
        )
        link_group_libs_debug_info = {}
        if impl_params.auto_link_group_specs != None:
            linked_link_groups = create_link_groups(
                ctx = ctx,
                link_groups = link_groups,
                link_strategy = link_strategy,
                linkable_graph = reduced_linkable_graph,
                link_group_mappings = link_group_mappings,
                link_group_preferred_linkage = link_group_preferred_linkage,
                executable_deps = exec_dep_roots,
                linker_flags = own_link_flags,
                link_group_specs = impl_params.auto_link_group_specs,
                other_roots = link_group_extra_link_roots,
                prefer_stripped_objects = impl_params.prefer_stripped_objects,
                anonymous = ctx.attrs.anonymous_link_groups,
                debug_linkable_extras = impl_params.generate_sub_targets.xcode_data,
                allow_cache_upload = impl_params.exe_allow_cache_upload,
                public_nodes = public_link_group_nodes,
                transformation_spec_context = transformation_spec_context,
                error_handler = impl_params.error_handler,
            )
            link_group_libs_debug_info = linked_link_groups.libs_debug_info
            for name, linked_link_group in linked_link_groups.libs.items():
                auto_link_groups[name] = linked_link_group.artifact
                if linked_link_group.library != None:
                    link_group_libs[name] = linked_link_group.library
                if linked_link_group.linker_map_data != None:
                    link_group_linker_map_data[name] = linked_link_group.linker_map_data
            own_exe_link_flags += linked_link_groups.symbol_ldflags
            targets_consumed_by_link_groups = linked_link_groups.targets_consumed_by_link_groups

        else:
            # NOTE(agallagher): We don't use version scripts and linker scripts
            # for non-auto-link-group flow, as it's note clear it's useful (e.g.
            # it's mainly for supporting dlopen-enabled libs and extensions).
            link_group_libs = gather_link_group_libs(
                children = [d.link_group_lib_info for d in link_deps],
            )

        pic_behavior = get_cxx_toolchain_info(ctx).pic_behavior

        roots = set(
            exec_dep_roots
            + find_relevant_roots(
                link_group = link_group,
                linkable_graph_node_map = linkable_graph_node_map,
                link_group_mappings = link_group_mappings,
                roots = link_group_extra_link_roots,
            ),
        )
        is_executable_link = True
        exec_linkables = collect_linkables(
            reduced_linkable_graph,
            is_executable_link,
            link_strategy,
            link_group_preferred_linkage,
            pic_behavior,
            roots,
        )

        build_context = BuildLinkGroupsContext(
            public_nodes = public_link_group_nodes,
            linkable_graph = reduced_linkable_graph,
            link_groups = link_groups,
            link_group_mappings = link_group_mappings,
            link_group_preferred_linkage = link_group_preferred_linkage,
            link_strategy = link_strategy,
            pic_behavior = pic_behavior,
            link_group_libs = {name: (lib.label, lib.shared_link_infos) for name, lib in link_group_libs.items()},
            prefer_stripped = impl_params.prefer_stripped_objects,
            prefer_optimized = False,
            transformation_spec_context = transformation_spec_context,
        )

        # TODO(T110378098): Similar to shared libraries, we need to identify all the possible
        # scenarios for which we need to propagate up link info and simplify this logic. For now
        # base which links to use based on whether link groups are defined.
        labels_to_links = get_filtered_labels_to_links_map(
            link_group,
            linkables = exec_linkables,
            is_executable_link = is_executable_link,
            build_context = build_context,
            force_static_follows_dependents = impl_params.link_groups_force_static_follows_dependents,
        )

        link_groups_binary_debug_info = LinkGroupsDebugLinkableItem(
            ordered_linkables = create_debug_linkable_entries(labels_to_links.map, root = None, with_extras = impl_params.generate_sub_targets.xcode_data),
        )
        link_groups_debug_info = LinkGroupsDebugLinkInfo(
            binary = link_groups_binary_debug_info,
            libs = link_group_libs_debug_info,
        )
        link_groups_libs_sub_targets = {}
        for name, lib in link_group_libs_debug_info.items():
            link_groups_libs_sub_targets[name] = [
                DefaultInfo(
                    default_output = ctx.actions.write_json(
                        ctx.label.name + ".link-groups-info.libs.{}.json".format(name),
                        lib,
                        has_content_based_path = False,
                    ),
                ),
            ]
        sub_targets[LINK_GROUPS_INFO_SUB_TARGET] = [
            DefaultInfo(
                default_output = ctx.actions.write_json(
                    ctx.label.name + ".link-groups-info.json",
                    link_groups_debug_info,
                    has_content_based_path = False,
                ),
                sub_targets = {
                    "bin": [
                        DefaultInfo(
                            default_output = ctx.actions.write_json(
                                ctx.label.name + ".link-groups-info.bin.json",
                                link_groups_binary_debug_info,
                                has_content_based_path = False,
                            ),
                        )
                    ],
                    "shared-libraries": [DefaultInfo(sub_targets = link_groups_libs_sub_targets)],
                },
            )
        ]

        if is_cxx_test and link_group != None:
            # if a cpp_unittest is part of the link group, we need to traverse through all deps
            # from the root again to ensure we link in gtest deps
            roots = set([d.linkable_graph.nodes.value.label for d in link_deps])
            exec_linkables = collect_linkables(
                linkable_graph_node_map,
                is_executable_link,
                link_strategy,
                link_group_preferred_linkage,
                pic_behavior,
                roots,
            )
            labels_to_links_to_merge = get_filtered_labels_to_links_map(
                link_group = None,
                linkables = exec_linkables,
                is_executable_link = False,
                build_context = build_context,
            )
            labels_to_links.map |= labels_to_links_to_merge.map

        # NOTE: Our Haskell DLL support impl currently links transitive haskell
        # deps needed by DLLs which get linked into the main executable as link-
        # whole.  To emulate this, we mark Haskell rules with a special label
        # and traverse this to find all the nodes we need to link whole.
        public_nodes = []
        if ctx.attrs.link_group_public_deps_label != None:
            public_nodes = get_transitive_deps_matching_labels(
                linkable_graph_node_map = linkable_graph_node_map,
                label = ctx.attrs.link_group_public_deps_label,
                roots = get_dedupped_roots_from_groups(link_group_info.groups.values()),
            )

        filtered_links = get_filtered_links(labels_to_links.map, set(public_nodes))
        filtered_targets = get_filtered_targets(labels_to_links.map)

        link_execution_preference = (
            LinkExecutionPreference("remote")
            if impl_params.force_remote_link_if_capable
            else get_resolved_cxx_binary_link_execution_preference(ctx, labels_to_links.map.keys(), impl_params.force_full_hybrid_if_capable)
        )

        # Unfortunately, link_groups does not use MergedLinkInfo to represent the args
        # for the resolved nodes in the graph.
        # Thus, we have no choice but to traverse all the nodes to dedupe the framework linker args.
        additional_links = apple_get_link_info_by_deduping_link_infos(ctx, filtered_links, frameworks_linkable, swiftmodule_linkable)
        if additional_links:
            filtered_links.append(additional_links)

        dep_links = LinkArgs(infos = filtered_links)
        sub_targets[LINK_GROUP_MAP_DATABASE_SUB_TARGET] = [get_link_group_map_json(ctx, filtered_targets)]

    gnu_use_link_groups = cxx_is_gnu(ctx) and len(link_group_mappings) > 0
    shlib_deps = _get_shared_library_symlink_deps(
        ctx,
        gnu_use_link_groups,
        impl_params,
        link_strategy,
        link_deps,
        linkable_graph,
    )

    shlib_info = merge_shared_libraries(ctx.actions, deps = shlib_deps)

    link_group_ctx = LinkGroupContext(
        link_group_mappings = link_group_mappings,
        link_group_libs = link_group_libs,
        link_group_preferred_linkage = link_group_preferred_linkage,
        labels_to_links_map = labels_to_links.map,
        targets_consumed_by_link_groups = targets_consumed_by_link_groups,
    )

    # Set up shared libraries symlink tree only when needed
    shared_libs = build_shared_libs_for_symlink_tree(
        gnu_use_link_groups,
        link_group_ctx,
        link_strategy,
        traverse_shared_library_info(shlib_info, transformation_provider = transformation_spec_context),
        impl_params.extra_shared_libs,
    )

    toolchain_info = get_cxx_toolchain_info(ctx)
    linker_info = toolchain_info.linker_info
    generated_build_info_external_debug_info = []
    binary_linker_flags = ctx.attrs.binary_linker_flags
    generated_build_info_args = []
    if getattr(ctx.attrs, "_generated_build_info_enabled", False):
        generated_build_info_args = [flag for flag in binary_linker_flags if _is_build_info_linker_flag(flag)]
        binary_linker_flags = [flag for flag in binary_linker_flags if not _is_build_info_linker_flag(flag)]
    generated_build_info_invalidation_inputs = (
        [out.object for out in cxx_outs]
        + [unpack_link_args(dep_links)]
        + [shared_lib.lib.output for shared_lib in shared_libs]
        + impl_params.generated_build_info_invalidation_inputs
    )
    generated_build_info_shared_library = None
    if generated_build_info_is_shared_library(ctx, link_groups_enabled = link_groups_enabled):
        binary_linker_flags += ["--build-info=none"]
        generated_build_info_shared_library = generate_build_info_shared_library(
            ctx,
            dep_links,
            generator_args = generated_build_info_args,
            invalidation_inputs = generated_build_info_invalidation_inputs,
            stable_placeholder = impl_params.link_stable_build_info_placeholder,
        )
        if generated_build_info_shared_library:
            shared_libs.append(generated_build_info_shared_library.library)
            sub_targets["generated_build_info"] = [
                DefaultInfo(default_output = generated_build_info_shared_library.library.lib.output),
            ]
    else:
        # Statically linked build info has no placeholder/swap step, so it
        # always uses the configured mode.
        generated_build_info = generate_build_info(
            ctx,
            generator_args = generated_build_info_args,
            invalidation_inputs = generated_build_info_invalidation_inputs,
        )
        if generated_build_info:
            generated_build_info_compile_output = compile_generated_build_info(ctx, generated_build_info)
            generated_build_info_external_debug_info = generated_build_info_compile_output.external_debug_info
            binary_linker_flags += generated_build_info_compile_output.objects + generated_build_info.linker_flags

    links = [
        LinkArgs(
            infos = [
                LinkInfo(
                    dist_thin_lto_codegen_flags = getattr(ctx.attrs, "dist_thin_lto_codegen_flags", []),
                    pre_flags = own_exe_link_flags,
                    linkables = [
                        ObjectsLinkable(
                            objects = [out.object for out in cxx_outs] + impl_params.extra_link_input,
                            linker_type = linker_info.type,
                            link_whole = True,
                        )
                    ],
                    external_debug_info = make_artifact_tset(
                        actions = ctx.actions,
                        label = ctx.label,
                        artifacts = (
                            [out.object for out in cxx_outs if out.object_has_external_debug_info]
                            + [out.external_debug_info for out in cxx_outs if out.external_debug_info != None]
                            + generated_build_info_external_debug_info
                            + (impl_params.extra_link_input if impl_params.extra_link_input_has_external_debug_info else [])
                        ),
                    ),
                ),
            ]
        ),
    ]
    if generated_build_info_shared_library:
        links.append(generated_build_info_shared_library.link_args)
    links += [dep_links] + impl_params.extra_link_args

    # If there are hidden dependencies to this target then add them as
    # hidden link args.
    if impl_params.extra_hidden:
        links.append(
            LinkArgs(flags = cmd_args(hidden = impl_params.extra_hidden)),
        )

    incremental_linking_mode = IncrementalLinkingMode(linker_info.incremental_linking)
    if impl_params.link_preference == LinkPreference("incremental"):
        incremental_link = incremental_linking_mode != IncrementalLinkingMode("disabled")
    elif impl_params.link_preference == LinkPreference("full"):
        incremental_link = False
    else:
        incremental_link = incremental_linking_mode == IncrementalLinkingMode("enabled")

    has_hip_device_debug = lazy.is_any(lambda out: out.hip_device_debug, cxx_outs) or link_args_have_hip_device_debug(links)
    link_result = _link_into_executable(
        ctx,
        # If shlib lib tree generation is enabled, pass in the shared libs (which
        # will trigger the necessary link tree and link args).
        shared_libs if impl_params.exe_shared_libs_link_tree else [],
        impl_params.executable_name,
        linker_info.binary_extension,
        binary_linker_flags,
        link_options(
            links = links,
            link_weight = linker_info.link_weight,
            link_ordering = map_val(LinkOrdering, ctx.attrs.link_ordering),
            link_execution_preference = link_execution_preference,
            enable_distributed_thinlto = ctx.attrs.enable_distributed_thinlto,
            strip = impl_params.strip_executable,
            strip_args_factory = impl_params.strip_args_factory,
            category_suffix = impl_params.exe_category_suffix,
            allow_cache_upload = impl_params.exe_allow_cache_upload,
            error_handler = impl_params.error_handler,
            extra_linker_outputs_factory = impl_params.extra_linker_outputs_factory,
            extra_linker_outputs_flags_factory = impl_params.extra_linker_outputs_flags_factory,
            extra_distributed_thin_lto_opt_outputs_merger = impl_params.extra_distributed_thin_lto_opt_outputs_merger,
            incremental_link = incremental_link,
            has_hip_device_debug = has_hip_device_debug,
        ),
        build_info_json = (generated_build_info_shared_library.json if generated_build_info_shared_library else None),
        link_groups_enabled = link_groups_enabled,
    )
    binary = link_result.exe
    runtime_files = link_result.runtime_files
    shared_libs_symlink_tree = link_result.shared_libs_symlink_tree
    linker_map_data = link_result.linker_map_data
    gc_sections_data = link_result.gc_sections_data

    # Define the xcode data sub target
    xcode_data_info = None
    if impl_params.generate_sub_targets.xcode_data:
        xcode_data_default_info, xcode_data_info = generate_xcode_data(
            ctx,
            rule_type = impl_params.rule_type,
            output = binary.output,
            populate_rule_specific_attributes_func = impl_params.cxx_populate_xcode_attributes_func,
            srcs = impl_params.srcs + impl_params.additional.srcs,
            argsfiles = compile_cmd_output.argsfiles.xcode,
            product_name = get_cxx_executable_product_name(ctx, has_hip_device_debug, link_groups_enabled = link_groups_enabled),
        )
        sub_targets[XCODE_DATA_SUB_TARGET] = xcode_data_default_info

    # Info about dynamic-linked libraries for fbpkg integration:
    # - the symlink dir that's part of RPATH
    # - sub-sub-targets that reference shared library dependencies and their respective dwp
    # - [shared-libraries] - a json map that references the above rules.
    if isinstance(shared_libs_symlink_tree, Artifact):
        sub_targets["rpath-tree"] = [
            DefaultInfo(
                default_output = shared_libs_symlink_tree,
                other_outputs = [shlib.lib.output for shlib in shared_libs] + [shlib.lib.dwp for shlib in shared_libs if shlib.lib.dwp],
            )
        ]

    # TODO(agallagher) There appears to be pre-existing soname conflicts
    # when building this (when using link groups), which prevents using
    # `with_unique_str_sonames`.
    str_soname_shlibs = {shlib.soname.ensure_str(): shlib for shlib in shared_libs if shlib.soname.is_str}

    readable_mappings = {}
    soname_to_group_mappings = {}
    if link_group_mappings:
        for node, group in link_group_mappings.items():
            soname = get_shared_library_name(linker_info, group, True)
            soname_to_group_mappings[soname] = group
            readable_mappings.setdefault(group, []).append(node.raw_target())

        sub_targets[LINK_GROUP_MAPPINGS_SUB_TARGET] = [
            DefaultInfo(
                default_output = ctx.actions.write_json(
                    binary.output.basename + LINK_GROUP_MAPPINGS_FILENAME_SUFFIX,
                    readable_mappings,
                    has_content_based_path = False,
                ),
            )
        ]

        linkable_graph_node_map = get_linkable_graph_node_map_func(linkable_graph)()
        sub_targets["binary_node_count"] = [
            DefaultInfo(
                default_output = ctx.actions.write_json(
                    binary.output.basename + ".binary_node_count.json",
                    {
                        "binary_node_count": len(linkable_graph_node_map),
                    },
                    has_content_based_path = False,
                ),
            )
        ]

    shared_libraries_sub_targets = {}
    for soname, shlib in str_soname_shlibs.items():
        targets = {}
        if shlib.lib.dwp:
            targets["dwp"] = [DefaultInfo(default_output = shlib.lib.dwp)]
        if shlib.lib.pdb:
            targets[PDB_SUB_TARGET] = get_pdb_providers(pdb = shlib.lib.pdb, binary = shlib.lib.output)

        group = soname_to_group_mappings.get(soname)
        if group in readable_mappings:
            output_json_file = binary.output.basename + "." + group + LINK_GROUP_MAPPINGS_FILENAME_SUFFIX
            targets[LINK_GROUP_MAPPINGS_SUB_TARGET] = [
                DefaultInfo(
                    default_output = ctx.actions.write_json(
                        output_json_file,
                        {group: readable_mappings[group]},
                        has_content_based_path = False,
                    ),
                )
            ]
        if group in link_group_linker_map_data:
            lm_data = link_group_linker_map_data[group]
            targets["linker-map"] = [DefaultInfo(default_output = lm_data.map, other_outputs = [lm_data.binary])]
        shared_libraries_sub_targets[soname] = [
            DefaultInfo(
                default_output = shlib.lib.output,
                sub_targets = targets,
            )
        ]

    sub_targets["shared-libraries"] = [
        DefaultInfo(
            default_output = ctx.actions.write_json(
                binary.output.basename + ".shared-libraries.json",
                {
                    "libraries": ["{}:{}[shared-libraries][{}]".format(ctx.label.path, ctx.label.name, soname) for soname in str_soname_shlibs],
                    "librariesdwp": [
                        "{}:{}[shared-libraries][{}][dwp]".format(ctx.label.path, ctx.label.name, soname)
                        for soname, shlib in str_soname_shlibs.items()
                        if shlib.lib.dwp
                    ],
                    "librariespdb": [
                        "{}:{}[shared-libraries][{}][pdb]".format(ctx.label.path, ctx.label.name, soname)
                        for soname, shlib in str_soname_shlibs.items()
                        if shlib.lib.pdb
                    ],
                    "rpathtree": ["{}:{}[rpath-tree]".format(ctx.label.path, ctx.label.name)] if shared_libs_symlink_tree else [],
                },
                has_content_based_path = False,
            ),
            sub_targets = shared_libraries_sub_targets,
        )
    ]

    # If we have some resources, write it to the resources JSON file and add
    # it and all resources to "runtime_files" so that we make to materialize
    # them with the final binary.
    resources = flatten_dict(
        gather_resources(
            label = ctx.label,
            resources = cxx_attr_resources(ctx),
            deps = cxx_attr_deps(ctx),
        ).values()
    )
    relocatable_resources_json = None
    relocatable_resources_contents = None
    if resources:
        runtime_files.append(
            create_resource_db(
                ctx = ctx,
                name = binary.output.basename + ".resources.json",
                binary = binary.output,
                resources = resources,
            )
        )
        for resource in resources.values():
            runtime_files.append(resource.default_output)
            runtime_files.extend(resource.other_outputs)
        relocatable_resources_json, relocatable_resources_contents = create_relocatable_resources_info(
            ctx = ctx,
            name = ctx.label.name,
            resources = resources,
        )

    if binary.dwp:
        # A `dwp` sub-target which generates the `.dwp` file for this binary and its shared lib dependencies.
        shared_libraries_dwp = [shlib.lib.dwp for shlib in shared_libs if shlib.lib.dwp] + (
            [link_result.dwp_symlink_tree] if link_result.dwp_symlink_tree else []
        )

        sub_targets["dwp"] = [
            DefaultInfo(
                default_output = binary.dwp,
                other_outputs = shared_libraries_dwp,
            ),
        ]

    # Own TU sidecars + transitive deps' sidecars.
    hip_all_debug_files = {}
    for out in cxx_outs:
        for arch, debug_file in out.hip_arch_debug_files.items():
            hip_all_debug_files.setdefault(arch, [])
            hip_all_debug_files[arch].append(debug_file)
    for arch, files in collect_hip_debug_from_links(links).items():
        hip_all_debug_files.setdefault(arch, [])
        hip_all_debug_files[arch].extend(files)

    if hip_all_debug_files:
        per_arch_debug_subs = {arch: [DefaultInfo(default_outputs = files)] for arch, files in hip_all_debug_files.items()}
        all_files = [f for files in hip_all_debug_files.values() for f in files]
        hip_debug_manifest = ctx.actions.write(
            "__hip_debug_manifest__.txt",
            cmd_args(all_files, delimiter = "\n"),
        )
        sub_targets["hip_debug"] = [
            DefaultInfo(
                default_output = hip_debug_manifest,
                other_outputs = all_files,
                sub_targets = per_arch_debug_subs,
            ),
        ]

    if binary.pdb:
        # A `pdb` sub-target which generates the `.pdb` file for this binary.
        sub_targets[PDB_SUB_TARGET] = get_pdb_providers(pdb = binary.pdb, binary = binary.output)

    if binary.ilk:
        sub_targets["ilk"] = [DefaultInfo(default_output = binary.ilk)]

    if toolchain_info.dumpbin_toolchain_path:
        sub_targets[DUMPBIN_SUB_TARGET] = get_dumpbin_providers(ctx, binary.output, toolchain_info.dumpbin_toolchain_path)

    # If bolt is not ran, binary.prebolt_output will be the same as binary.output. Only
    # expose binary.prebolt_output if cxx_use_bolt(ctx) is True to avoid confusion
    if cxx_use_bolt(ctx):
        sub_targets["prebolt"] = [DefaultInfo(default_output = binary.prebolt_output)]

    if linker_map_data:
        sub_targets["linker-map"] = [DefaultInfo(default_output = linker_map_data.map, other_outputs = [linker_map_data.binary])]

    if gc_sections_data:
        sub_targets["gc-sections"] = [DefaultInfo(default_output = gc_sections_data.gc_sections, other_outputs = [gc_sections_data.binary])]

    sub_targets["linker.argsfile"] = [
        DefaultInfo(
            default_output = binary.linker_argsfile,
        )
    ]

    link_cmd_debug_output = make_link_command_debug_output(binary)
    if link_cmd_debug_output != None:
        link_cmd_debug_output_file = make_link_command_debug_output_json_info(ctx.actions, [link_cmd_debug_output])
        sub_targets["linker.command"] = [
            DefaultInfo(
                default_outputs = filter(None, [link_cmd_debug_output_file]),
            )
        ]

    if linker_info.supports_distributed_thinlto and ctx.attrs.enable_distributed_thinlto:
        sub_targets["index.argsfile"] = [
            DefaultInfo(
                default_output = binary.index_argsfile,
            )
        ]

    # Provide a debug info target to make sure unpacked external debug info
    # (dwo) is materialized.
    external_debug_info = make_artifact_tset(
        actions = ctx.actions,
        children = ([binary.external_debug_info] + [s.lib.external_debug_info for s in shared_libs] + impl_params.additional.static_external_debug_info),
    )
    external_debug_info_artifacts = project_artifacts(ctx.actions, external_debug_info)
    materialize_external_debug_info = ctx.actions.write(
        "debuginfo.artifacts",
        external_debug_info_artifacts,
        with_inputs = True,
        has_content_based_path = False,
    )
    sub_targets["debuginfo"] = [
        DefaultInfo(
            default_output = materialize_external_debug_info,
        )
    ]
    sub_targets["debug_coverage_instrumentation"] = [
        DefaultInfo(
            default_output = materialize_external_debug_info,
        )
    ]

    sub_targets["exe"] = [
        DefaultInfo(
            default_output = binary.output,
            other_outputs = runtime_files,
        )
    ]

    for additional_subtarget, subtarget_providers in impl_params.additional.subtargets.items():
        sub_targets[additional_subtarget] = subtarget_providers

    sub_targets.update(link_result.extra_outputs)

    xplugins_debug_artifacts_info = xplugins_get_debug_artifacts_info(ctx, cxx_deps)

    return CxxExecutableOutput(
        binary = binary.output,
        unstripped_binary = binary.unstripped_output,
        dwp = binary.dwp,
        runtime_files = runtime_files,
        sub_targets = sub_targets,
        link_args = links,
        external_debug_info = external_debug_info,
        external_debug_info_artifacts = external_debug_info_artifacts,
        shared_libs = shared_libs,
        auto_link_groups = auto_link_groups,
        compilation_db = comp_db_info,
        xcode_data = xcode_data_info,
        linker_map_data = linker_map_data,
        gc_sections_data = gc_sections_data,
        link_command_debug_output = link_cmd_debug_output,
        dist_info = DistInfo(
            shared_libs = shlib_info.set,
            nondebug_runtime_files = runtime_files,
            relocatable_resources_json = relocatable_resources_json,
            relocatable_resources_contents = relocatable_resources_contents,
        ),
        sanitizer_runtime_files = link_result.sanitizer_runtime_files,
        index_stores = index_stores,
        diagnostics = all_diagnostics,
        validation_specs = get_attrs_validation_specs(ctx),
        gcno_files = dedupe(gcno_files),
        xplugins_debug_artifacts_info = xplugins_debug_artifacts_info,
        build_info_manifest_entries = (generated_build_info_shared_library.manifest_entries if generated_build_info_shared_library else None),
    )

_CxxLinkExecutableResult = record(
    # The resulting executable
    exe = LinkedObject,
    # Files that must be present for the executable to run successfully. These
    # are always materialized, whether the executable is the output of a build
    # or executed as a host tool.
    runtime_files = list[ArgLike],
    # Files needed to debug the executable. These need to be materialized when
    # this executable is the output of a build, but not when it is used by other
    # rules.
    external_debug_info = list[TransitiveSetArgsProjection],
    dwp_symlink_tree = field(list[Artifact] | Artifact | None),
    # Optional shared libs symlink tree symlinked_dir action
    shared_libs_symlink_tree = [list[Artifact], Artifact, None],
    linker_map_data = [CxxLinkerMapData, None],
    gc_sections_data = [CxxGcSectionsData, None],
    sanitizer_runtime_files = list[Artifact],
    # Extra output providers produced by extra_linker_outputs_factory
    extra_outputs = dict[str, list[DefaultInfo]],
)

def _get_shared_library_symlink_deps(
    ctx: AnalysisContext,
    gnu_use_link_groups: bool,
    impl_params: CxxRuleConstructorParams,
    link_strategy: LinkStrategy,
    link_deps: list[LinkableProviders],
    linkable_graph: LinkableGraph,
) -> list[SharedLibraryInfo]:
    """
    Determine which shared library deps need a symlink tree alongside the executable.
    """

    # Only setup a shared library symlink tree when shared linkage or link_groups is used
    shlib_deps = []
    if link_strategy == LinkStrategy("shared") or gnu_use_link_groups:
        shlib_deps = [d.shared_library_info for d in link_deps] + [d.shared_library_info for d in impl_params.extra_link_roots]
    elif impl_params.runtime_dependency_handling == RuntimeDependencyHandling("no_symlink"):
        # Do not create a shared library symlink tree alongside the executable.
        pass
    elif impl_params.runtime_dependency_handling == RuntimeDependencyHandling("symlink"):
        # Include all transitive runtime shared library deps in a symlink tree.
        for linkable_node in linkable_graph.nodes.traverse():
            if linkable_node.linkable == None:
                continue
            preferred_linkage = linkable_node.linkable.preferred_linkage
            output_style = get_lib_output_style(link_strategy, preferred_linkage, PicBehavior("supported"))
            if output_style == LibOutputStyle("shared_lib") and not linkable_node.linkable.stub:
                shlib_deps.append(merge_shared_libraries(ctx.actions, node = linkable_node.linkable.shared_libs))
    elif impl_params.runtime_dependency_handling == RuntimeDependencyHandling("symlink_single_level_only"):
        # Include only first-level (direct) runtime shared library deps in a symlink tree.
        for d in link_deps + impl_params.extra_link_roots:
            if d.linkable_graph == None:
                continue
            preferred_linkage = d.linkable_graph.nodes.value.linkable.preferred_linkage
            output_style = get_lib_output_style(link_strategy, preferred_linkage, PicBehavior("supported"))
            if output_style == LibOutputStyle("shared_lib"):
                shlib_deps.append(d.shared_library_info)

    return shlib_deps

def _link_into_executable(
    ctx: AnalysisContext,
    shared_libs: list[SharedLibrary],
    executable_name: [str, None],
    binary_extension: str,
    binary_linker_flags: list[typing.Any],
    opts: LinkOptions,
    build_info_json: Artifact | None = None,
    link_groups_enabled: bool = False,
) -> _CxxLinkExecutableResult:
    if executable_name and binary_extension and executable_name.endswith(binary_extension):
        # don't append .exe if it already is .exe
        output_name = executable_name
    else:
        output_name = "{}{}".format(
            executable_name if executable_name else get_cxx_executable_product_name(ctx, opts.has_hip_device_debug, link_groups_enabled = link_groups_enabled),
            "." + binary_extension if binary_extension else "",
        )
    output = ctx.actions.declare_output(output_name, has_content_based_path = False)
    executable_args = executable_shared_lib_arguments(
        ctx,
        get_cxx_toolchain_info(ctx),
        output,
        shared_libs,
    )

    link_result = cxx_link_into(
        ctx = ctx,
        output = output,
        result_type = CxxLinkResultType("executable"),
        # Binary linker flags should come after default link flags so they can override default settings.
        opts = merge_link_options(
            opts,
            binary_links = [
                LinkArgs(
                    infos = [
                        LinkInfo(
                            pre_flags = binary_linker_flags,
                        ),
                    ]
                ),
            ],
            links = [LinkArgs(flags = executable_args.extra_link_args)] + opts.links,
        ),
        build_info_json = build_info_json,
        link_groups_enabled = link_groups_enabled,
    )

    return _CxxLinkExecutableResult(
        exe = link_result.linked_object,
        runtime_files = executable_args.runtime_files + link_result.sanitizer_runtime_files,
        external_debug_info = executable_args.external_debug_info,
        shared_libs_symlink_tree = executable_args.shared_libs_symlink_tree,
        dwp_symlink_tree = executable_args.dwp_symlink_tree,
        linker_map_data = link_result.linker_map_data,
        gc_sections_data = link_result.gc_sections_data,
        sanitizer_runtime_files = link_result.sanitizer_runtime_files,
        extra_outputs = link_result.extra_outputs if link_result.extra_outputs else {},
    )

def get_cxx_post_link_suffix(ctx: AnalysisContext, has_hip_device_debug: bool = False, link_groups_enabled: bool = False) -> str:
    """
    The suffix the linker's output must carry so that each post-link stage of
    `cxx_link_into` can strip the part it owns, leaving the canonical name.

    Appended in reverse of the order the stages run, so the outermost suffix
    belongs to the stage that runs first.
    """
    suffix = ""
    if cxx_stamp_build_info(ctx, link_groups_enabled = link_groups_enabled):
        suffix += PRE_STAMPED_SUFFIX
    if get_elf_sections(ctx):
        suffix += PRE_ADD_ELF_SECTIONS_SUFFIX
    if has_hip_device_debug and hip_debug_extract_available(get_cxx_toolchain_info(ctx)):
        suffix += PRE_EXTRACT_SUFFIX
    if cxx_use_bolt(ctx):
        suffix += PRE_BOLT_SUFFIX
    return suffix

def get_cxx_executable_product_name(ctx: AnalysisContext, has_hip_device_debug: bool = False, link_groups_enabled: bool = False) -> str:
    return ctx.label.name + get_cxx_post_link_suffix(ctx, has_hip_device_debug, link_groups_enabled = link_groups_enabled)

def _is_build_info_linker_flag(flag) -> bool:
    return flag.startswith("--build-info")
