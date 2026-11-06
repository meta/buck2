# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load(
    "@prelude//:resources.bzl",
    "gather_resources",
    "make_resource_info",
)
load(
    "@prelude//android:android_providers.bzl",
    "merge_android_packageable_info",
)
load("@prelude//apple:resource_groups.bzl", "create_resource_graph")
load("@prelude//cxx:attrs.bzl", "cxx_binary_and_test_attrs", "cxx_binary_base_attrs", "cxx_common", "cxx_library_base_attrs", "cxx_precompiled_header_attrs")
load(
    "@prelude//cxx:compile_types.bzl",
    "UseHeaderUnitsMode",
)
load(
    "@prelude//cxx:cuda.bzl",
    "CudaCompileStyle",
)
load("@prelude//cxx:cxx_flags.bzl", "cxx_attr_flags")
load("@prelude//cxx:cxx_sources.bzl", "get_srcs_with_flags")
load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "RuntimeDependencyHandling",
    "ShlibInterfacesMode",
)
load("@prelude//cxx:cxx_utility.bzl", "cxx_attrs_get_allow_cache_upload", "cxx_attrs_use_fbcc_rust_wrapper")
load(
    "@prelude//cxx:link_groups_types.bzl",
    "LINK_GROUP_MAP_ATTR",
    "LinkGroupInfo",  # @unused Used as a type
)
# @oss-disable[end= ]: load("@prelude//cxx/meta_only:linker_outputs.bzl", "get_extra_linker_output_flags", "get_extra_linker_outputs")
load("@prelude//decls:common.bzl", "CxxRuntimeType", "CxxSourceType", "HeadersAsRawHeadersMode", "IncludeType", "buck")
load("@prelude//decls:native_common.bzl", "native_common")
load("@prelude//decls:re_test_common.bzl", "re_test_common")
load("@prelude//decls:test_common.bzl", "test_common")
load("@prelude//decls:third_party_common.bzl", "third_party_common")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//graphql:graphql.bzl", "graphql_providers")
load("@prelude//linking:execution_preference.bzl", "LinkExecutionPreference")
load(
    "@prelude//linking:link_groups.bzl",
    "merge_link_group_lib_info",
)
load(
    "@prelude//linking:link_info.bzl",
    "Archive",
    "ArchiveLinkable",
    "CxxSanitizerRuntimeInfo",
    "LibOutputStyle",
    "LinkArgs",
    "LinkCommandDebugOutputInfo",
    "LinkInfo",
    "LinkInfos",
    "LinkStrategy",
    "LinkedObject",
    "SharedLibLinkable",
    "create_merged_link_info",
    "create_merged_link_info_for_propagation",
    "get_lib_output_style",
    "get_link_args_for_strategy",
    "get_output_styles_for_linkage",
    "subtarget_for_output_style",
    "to_link_strategy",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "DLOPENABLE_LIBRARY_INFO_MARKER",
    "DlopenableLibraryInfo",
    "LinkableGraph",
    "create_linkable_graph",
    "create_linkable_graph_node",
    "create_linkable_node",
)
load(
    "@prelude//linking:linkables.bzl",
    "linkables",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraries",
    "SharedLibraryInfo",
    "create_shlib_from_ctx",
    "extract_soname_from_shlib",
    "merge_shared_libraries",
    "to_soname",
)
load("@prelude//linking:strip.bzl", "strip_debug_info")
load("@prelude//linking:types.bzl", "LinkPreference", "Linkage")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")
load("@prelude//python:manifest.bzl", "create_manifest_for_entries")
load("@prelude//target_stats:target_stats.bzl", "CycleMode", "target_stats_providers_and_subtargets")
load("@prelude//target_stats:target_stats_config.bzl", "TARGET_STATS_ENABLED")
load("@prelude//test:inject_test_run_info.bzl", "inject_test_run_info")
load(
    "@prelude//tests:re_utils.bzl",
    "get_re_executors_from_props",
)
load(
    "@prelude//third-party:build.bzl",
    "create_third_party_build_info",
)
load(
    "@prelude//third-party:providers.bzl",
    "ThirdPartyBuildInfo",
)
load("@prelude//transitions:constraint_overrides.bzl", "constraint_overrides")
load("@prelude//unix:providers.bzl", "UnixEnv", "create_unix_env_info")
load("@prelude//utils:expect.bzl", "expect")
load(
    "@prelude//utils:utils.bzl",
    "filter_and_map_idx",
    "value_or",
)
load(":attrs.bzl", "BUILD_INFO_ATTR", "cxx_library_extra_attrs")
load(":clang_profile_list.bzl", "get_coverage_profile_list_srcs")
load(":cxx_context.bzl", "get_cxx_toolchain_info")
load(":cxx_executable.bzl", "cxx_executable")
load(
    ":cxx_library.bzl",
    "CxxLibraryOutput",  # @unused Used as a type
    "cxx_library_parameterized",
)
load(
    ":cxx_library_utility.bzl",
    "cxx_attr_dep_metadata",
    "cxx_attr_deps",
    "cxx_attr_exported_deps",
    "cxx_attr_link_style",
    "cxx_attr_linker_flags_all",
    "cxx_attr_preferred_linkage",
    "cxx_attr_use_content_based_paths",
    "cxx_inherited_link_info",
    "cxx_platform_supported",
    "cxx_use_shlib_intfs_mode",
)
load(":cxx_types.bzl", "CxxRuleConstructorParams", "CxxRuleProviderParams", "CxxRuleSubTargetParams", "xcode_data_enabled")
load(":gcno.bzl", "GcnoFilesInfo")
load(
    ":groups_types.bzl",
    "Group",
    "MATCH_ALL_LABEL",
    "NO_MATCH_LABEL",
)
load(
    ":headers.bzl",
    "CPrecompiledHeaderInfo",
    "cxx_attr_headers_list",
    "cxx_get_regular_cxx_headers_layout",
)
load(
    ":link.bzl",
    "cxx_link_shared_library",
)
load(
    ":link_groups.bzl",
    "LinkGroupLibSpec",
    "get_link_group_info",
)
load(
    ":link_types.bzl",
    "link_options",
)
load(
    ":linker.bzl",
    "DUMPBIN_SUB_TARGET",
    "IMPORT_LIBRARY_SUB_TARGET",
    "PDB_SUB_TARGET",
    "get_dumpbin_providers",
    "get_link_whole_args",
    "get_pdb_providers",
    "get_shared_library_name",
    "get_shared_library_name_for_param",
    "sandbox_exported_linker_flags",
)
load(
    ":omnibus.bzl",
    "create_linkable_root",
)
load(
    ":preprocessor.bzl",
    "CPreprocessor",
    "CPreprocessorArgs",
    "cxx_exported_preprocessor_info",
    "cxx_inherited_preprocessor_infos",
    "cxx_merge_cpreprocessors",
    "format_system_include_arg",
)
load(
    ":shared_library_interface.bzl",
    "shared_library_interface",
)

#####################################################################
# Operations

def _get_use_header_units_mode(use_header_units: bool | str) -> UseHeaderUnitsMode:
    """Convert the use_header_units attribute to UseHeaderUnitsMode enum."""
    if use_header_units == True:
        return UseHeaderUnitsMode("pcm")
    elif use_header_units == "stub":
        return UseHeaderUnitsMode("stub")
    else:
        return UseHeaderUnitsMode("none")

def _get_shared_link_style_sub_targets_and_providers(
    output_style: LibOutputStyle, ctx: AnalysisContext, output: [CxxLibraryOutput, None]
) -> (dict[str, list[Provider]], list[Provider]):
    if output_style != LibOutputStyle("shared_lib") or output == None:
        return ({}, [])
    sub_targets = {}
    providers = []
    if output.dwp != None:
        sub_targets["dwp"] = [DefaultInfo(default_output = output.dwp)]
    if output.hip_arch_debug_files:
        all_files = [f for files in output.hip_arch_debug_files.values() for f in files]
        sub_targets["hip_debug"] = [
            DefaultInfo(
                default_outputs = all_files,
                sub_targets = {arch: [DefaultInfo(default_outputs = files)] for arch, files in output.hip_arch_debug_files.items()},
            ),
        ]
    if output.pdb != None:
        sub_targets[PDB_SUB_TARGET] = get_pdb_providers(pdb = output.pdb, binary = output.default)
    cxx_toolchain = get_cxx_toolchain_info(ctx)
    if cxx_toolchain.dumpbin_toolchain_path != None:
        sub_targets[DUMPBIN_SUB_TARGET] = get_dumpbin_providers(ctx, output.default, cxx_toolchain.dumpbin_toolchain_path)
    if output.linker_map != None:
        sub_targets["linker-map"] = [DefaultInfo(default_output = output.linker_map.map, other_outputs = [output.linker_map.binary])]
    if output.implib != None:
        sub_targets[IMPORT_LIBRARY_SUB_TARGET] = [DefaultInfo(default_output = output.implib)]
    return (sub_targets, providers)

def cxx_library_impl(ctx: AnalysisContext) -> list[Provider]:
    return cxx_library_generate(ctx, "cxx_library")

def cxx_library_generate(ctx: AnalysisContext, rule_type: str) -> list[Provider]:
    if ctx.attrs.can_be_asset and ctx.attrs.used_by_wrap_script:
        fail("Cannot use `can_be_asset` and `used_by_wrap_script` in the same rule")

    if ctx.attrs._is_building_android_binary:
        sub_target_params, provider_params = _get_params_for_android_binary_cxx_library()
    else:
        sub_target_params = CxxRuleSubTargetParams(xcode_data = xcode_data_enabled())
        provider_params = CxxRuleProviderParams(
            third_party_build = True,
        )

    params = CxxRuleConstructorParams(
        rule_type = rule_type,
        headers_layout = cxx_get_regular_cxx_headers_layout(ctx),
        srcs = get_srcs_with_flags(ctx),
        output_style_sub_targets_and_providers_factory = _get_shared_link_style_sub_targets_and_providers,
        generate_sub_targets = sub_target_params,
        generate_providers = provider_params,
        compiler_flags = ctx.attrs.compiler_flags,
        cxx_flags = cxx_attr_flags(ctx),
        lang_compiler_flags = ctx.attrs.lang_compiler_flags,
        preprocessor_flags = ctx.attrs.preprocessor_flags,
        lang_preprocessor_flags = ctx.attrs.lang_preprocessor_flags,
        use_header_units = _get_use_header_units_mode(ctx.attrs.use_header_units),
        export_header_unit = ctx.attrs.export_header_unit,
        export_header_unit_filter = ctx.attrs.export_header_unit_filter,
        error_handler = get_cxx_toolchain_info(ctx).cxx_error_handler,
        extra_dwp_flags = ctx.attrs.extra_dwp_flags,
        use_fbcc_rust_wrapper = cxx_attrs_use_fbcc_rust_wrapper(ctx.attrs),
        allow_cache_upload = cxx_attrs_get_allow_cache_upload(ctx.attrs, get_cxx_toolchain_info(ctx).cxx_compiler_info.allow_cache_upload),
        precompiled_header = ctx.attrs.precompiled_header,
        prefix_header = ctx.attrs.prefix_header,
        _cxx_toolchain = ctx.attrs._cxx_toolchain,
        use_content_based_paths = cxx_attr_use_content_based_paths(ctx),
        coverage_instrumentation_compiler_flags = ctx.attrs.coverage_instrumentation_compiler_flags,
        coverage_profile_list = ctx.attrs.coverage_profile_list[DefaultInfo].default_outputs[0] if ctx.attrs.coverage_profile_list else None,
        coverage_profile_list_srcs = get_coverage_profile_list_srcs(ctx.attrs.coverage_profile_list),
        separate_debug_info = ctx.attrs.separate_debug_info,
        cuda_compile_style = CudaCompileStyle(ctx.attrs.cuda_compile_style),
        # @oss-disable[end= ]: extra_linker_outputs_factory = get_extra_linker_outputs,
        # @oss-disable[end= ]: extra_linker_outputs_flags_factory = get_extra_linker_output_flags,
        supports_stripping = ctx.attrs.supports_stripping,
        expect_eligible_for_dedupe = getattr(ctx.attrs, "expect_eligible_for_dedupe", False),
    )
    output = cxx_library_parameterized(ctx, params)
    return output.providers + graphql_providers(ctx)

def _only_shared_mappings(group: Group) -> bool:
    """
    Return whether this group only has explicit "shared" linkage mappings,
    which indicates a group that re-uses pre-linked libs.
    """
    for mapping in group.mappings:
        if mapping.preferred_linkage != Linkage("shared"):
            return False
    return True

def create_shared_lib_link_group_specs(ctx: AnalysisContext, link_group_definitions: list[Group]) -> list[LinkGroupLibSpec]:
    specs = []
    linker_info = get_cxx_toolchain_info(ctx).linker_info
    for group in link_group_definitions:
        if group.name in (MATCH_ALL_LABEL, NO_MATCH_LABEL):
            continue

        # TODO(agallagher): We should probably add proper handling for "provided"
        # system handling to avoid needing this special case.
        if _only_shared_mappings(group):
            continue
        specs.append(
            LinkGroupLibSpec(
                name = get_shared_library_name(linker_info, group.name, apply_default_prefix = True),
                is_shared_lib = True,
                group = group,
            ),
        )
    return specs

def get_auto_link_group_specs(ctx: AnalysisContext, link_group_info: [LinkGroupInfo, None]) -> [list[LinkGroupLibSpec], None]:
    if link_group_info == None or not ctx.attrs.auto_link_groups:
        return None
    return create_shared_lib_link_group_specs(ctx, link_group_info.groups.values())

def cxx_binary_impl(ctx: AnalysisContext) -> list[Provider]:
    link_strategy = to_link_strategy(cxx_attr_link_style(ctx))
    link_group_info = get_link_group_info(
        ctx,
        filter_and_map_idx(LinkableGraph, cxx_attr_deps(ctx)),
        link_strategy,
    )
    params = CxxRuleConstructorParams(
        rule_type = "cxx_binary",
        generate_sub_targets = CxxRuleSubTargetParams(xcode_data = xcode_data_enabled()),
        executable_name = ctx.attrs.executable_name,
        headers_layout = cxx_get_regular_cxx_headers_layout(ctx),
        srcs = get_srcs_with_flags(ctx),
        link_group_info = link_group_info,
        auto_link_group_specs = get_auto_link_group_specs(ctx, link_group_info),
        prefer_stripped_objects = ctx.attrs.prefer_stripped_objects,
        exe_allow_cache_upload = cxx_attrs_get_allow_cache_upload(ctx.attrs),
        force_remote_link_if_capable = ctx.attrs.link_execution_preference == "remote",
        extra_link_roots = linkables(ctx.attrs.link_group_deps),
        compiler_flags = ctx.attrs.compiler_flags,
        cxx_flags = cxx_attr_flags(ctx),
        lang_compiler_flags = ctx.attrs.lang_compiler_flags,
        preprocessor_flags = ctx.attrs.preprocessor_flags,
        lang_preprocessor_flags = ctx.attrs.lang_preprocessor_flags,
        use_header_units = _get_use_header_units_mode(ctx.attrs.use_header_units),
        runtime_dependency_handling = RuntimeDependencyHandling(ctx.attrs.runtime_dependency_handling)
        if ctx.attrs.runtime_dependency_handling
        else get_cxx_toolchain_info(ctx).runtime_dependency_handling,
        error_handler = get_cxx_toolchain_info(ctx).cxx_error_handler,
        extra_dwp_flags = ctx.attrs.extra_dwp_flags,
        use_fbcc_rust_wrapper = cxx_attrs_use_fbcc_rust_wrapper(ctx.attrs),
        allow_cache_upload = cxx_attrs_get_allow_cache_upload(ctx.attrs, get_cxx_toolchain_info(ctx).cxx_compiler_info.allow_cache_upload),
        precompiled_header = ctx.attrs.precompiled_header,
        prefix_header = ctx.attrs.prefix_header,
        _cxx_toolchain = ctx.attrs._cxx_toolchain,
        use_content_based_paths = cxx_attr_use_content_based_paths(ctx),
        coverage_instrumentation_compiler_flags = ctx.attrs.coverage_instrumentation_compiler_flags,
        coverage_profile_list = ctx.attrs.coverage_profile_list[DefaultInfo].default_outputs[0] if ctx.attrs.coverage_profile_list else None,
        coverage_profile_list_srcs = get_coverage_profile_list_srcs(ctx.attrs.coverage_profile_list),
        separate_debug_info = ctx.attrs.separate_debug_info,
        cuda_compile_style = CudaCompileStyle(ctx.attrs.cuda_compile_style),
        link_preference = LinkPreference(ctx.attrs.link_preference),
        # @oss-disable[end= ]: extra_linker_outputs_factory = get_extra_linker_outputs,
        # @oss-disable[end= ]: extra_linker_outputs_flags_factory = get_extra_linker_output_flags,
    )
    output = cxx_executable(ctx, params)

    extra_providers = []
    if output.link_command_debug_output:
        extra_providers.append(LinkCommandDebugOutputInfo(debug_outputs = [output.link_command_debug_output]))
    if output.sanitizer_runtime_files:
        extra_providers.append(CxxSanitizerRuntimeInfo(runtime_files = output.sanitizer_runtime_files))
    if output.validation_specs:
        extra_providers.append(ValidationInfo(validations = output.validation_specs))
    if output.xplugins_debug_artifacts_info:
        extra_providers.append(output.xplugins_debug_artifacts_info)

    if get_cxx_toolchain_info(ctx).gcno_files and output.gcno_files:
        extra_providers.append(GcnoFilesInfo(gcno_files = output.gcno_files))

    # Unix env provider.
    extra_providers.append(
        create_unix_env_info(
            actions = ctx.actions,
            env = UnixEnv(
                label = ctx.label,
                binaries = [
                    create_manifest_for_entries(
                        ctx = ctx,
                        name = "unix_env",
                        entries = [
                            (ctx.label.name, output.binary, ""),
                        ],
                    ),
                ],
            ),
            # TODO(agallagher): We only want to traverse deps when dynamically
            # linking.
            # deps = ctx.attrs.deps,
        ),
    )

    # When an executable is the output of a build, also materialize all the
    # unpacked external debuginfo that goes with it. This makes `buck2 build
    # :main` equivalent to `buck2 build :main :main[debuginfo]`.
    #
    # This is wasted work if we are building an executable together with its dwp
    # subtarget (`buck2 build :main :main[dwp]`) in which case a large number of
    # unpacked debuginfo files can end up being materialized redundantly. LLDB
    # will ignore them and obtain debuginfo via the single packed debuginfo file
    # instead.
    #
    # But materializing unpacked debuginfo is usually the right tradeoff
    # because it means the output of `buck2 build :main` is always immediately
    # usable in a debugger. Toolchains whose debugging workflow materializes
    # debuginfo on demand instead (via the `[debuginfo]` or `[dwp]`
    # sub-targets) can opt out with `materialize_external_debug_info = False`.
    #
    # External debuginfo is *not* materialized when an executable is depended on
    # by another rule, such as by $(exe ...) or exec_dep.
    other_outputs = output.runtime_files + (output.external_debug_info_artifacts if get_cxx_toolchain_info(ctx).materialize_external_debug_info else [])

    return (
        filter(
            None,
            [
                DefaultInfo(
                    default_output = output.binary,
                    other_outputs = other_outputs,
                    sub_targets = output.sub_targets,
                ),
                RunInfo(args = cmd_args(output.binary, hidden = output.runtime_files)),
                output.compilation_db,
                output.xcode_data,
                output.dist_info,
            ],
        )
        + extra_providers
    )

def _prebuilt_item(_ctx: AnalysisContext, item: [typing.Any, None]) -> [typing.Any, None]:
    """
    Return the item if it's set.
    """
    return item

def _prebuilt_linkage(ctx: AnalysisContext) -> Linkage:
    """
    Construct the preferred linkage to use for the given prebuilt library.
    """
    if ctx.attrs.header_only:
        return Linkage("any")
    if ctx.attrs.force_static:
        return Linkage("static")
    preferred_linkage = cxx_attr_preferred_linkage(ctx)
    if preferred_linkage != Linkage("any"):
        return preferred_linkage
    if ctx.attrs.provided:
        return Linkage("shared")
    return Linkage("any")

def _create_prebuilt_library_outputs(
    ctx: AnalysisContext,
    static_lib,
    static_pic_lib,
    shared_lib,
    static_lib_stripped,
    static_pic_lib_stripped,
    soname,
    linker_type,
    linker_flags,
    inherited_exported_link,
    header_dirs,
    propagated_preprocessor,
    propagate_shared_libs: bool,
):
    """
    Create outputs for all output styles of a prebuilt library.

    Returns a struct with:
      - outputs: dict of output_style -> artifact
      - libraries: dict of output_style -> LinkInfos
      - solibs: list of shared libraries
      - sub_targets: dict of sub-target names -> providers
      - generated_shared_lib: whether the shared lib was created via a link rule

    Args:
        propagate_shared_libs: Whether to propagate shared libraries via SharedLibraryInfo.
            Should be True only if the library's preferred linkage allows shared linking.
    """
    linker_info = get_cxx_toolchain_info(ctx).linker_info

    # Gather link infos, outputs, and shared libs for all output styles.
    outputs = {}
    libraries = {}
    solibs = []
    sub_targets = {}
    generated_shared_lib = False  # whether the shared lib is created via a link rule

    # Generate outputs for ALL output styles (not just those for a specific preferred_linkage)
    for output_style in [LibOutputStyle("archive"), LibOutputStyle("pic_archive"), LibOutputStyle("shared_lib")]:
        out = None
        linkable = None
        linkable_stripped = None

        # If we have sources to compile, generate the necessary libraries and
        # add them to the exported link info.
        if not ctx.attrs.header_only:

            def archive_linkable(lib):
                return ArchiveLinkable(
                    archive = Archive(artifact = lib),
                    linker_type = linker_type,
                    link_whole = ctx.attrs.link_whole,
                    supports_lto = ctx.attrs.supports_lto,
                )

            if output_style == LibOutputStyle("archive"):
                if static_lib:
                    out = static_lib
                    linkable = archive_linkable(static_lib)
                    linkable_stripped = archive_linkable(static_lib_stripped) if static_lib_stripped else None
            elif output_style == LibOutputStyle("pic_archive"):
                lib = static_pic_lib or static_lib
                if lib:
                    out = lib
                    linkable = archive_linkable(lib)
                    linkable_stripped = (
                        archive_linkable(static_pic_lib_stripped or static_lib_stripped) if (static_pic_lib_stripped or static_lib_stripped) else None
                    )
            else:  # shared
                # If no shared library was provided, link one from the static libraries.
                if shared_lib != None:
                    shared_lib = LinkedObject(output = shared_lib, unstripped_output = shared_lib)
                else:
                    lib = static_pic_lib or static_lib
                    if lib:
                        shlink_args = []

                        # TODO(T110378143): Support post link flags properly.
                        shlink_args.extend(linker_flags.exported_flags)
                        shlink_args.extend(linker_flags.flags)
                        shlink_args.extend(get_link_whole_args(linker_type, [lib]))

                        # TODO(T110378118): As per v1, we always link against "shared"
                        # dependencies when building a shaerd library.
                        shared_link_args = get_link_args_for_strategy(
                            ctx.actions,
                            ctx.label,
                            linker_info,
                            inherited_exported_link,
                            LinkStrategy("shared"),
                            prefer_stripped = False,
                            transformation_spec_context = None,
                        )
                        link_result = cxx_link_shared_library(
                            ctx = ctx,
                            output = soname.ensure_str(),
                            name = soname.ensure_str(),
                            opts = link_options(
                                links = [
                                    LinkArgs(flags = shlink_args),
                                    shared_link_args,
                                ],
                                link_execution_preference = LinkExecutionPreference("any"),
                            ),
                        )
                        shared_lib = link_result.linked_object
                        generated_shared_lib = True

                if shared_lib:
                    out = shared_lib.output

                    shared_lib_for_linking = shared_lib.output

                    # Generate a shared library interface if the rule supports it.
                    # TODO: We should not permit both modes, but to keep builds green we have to for now.
                    if ctx.attrs.supports_shared_library_interface and (
                        cxx_use_shlib_intfs_mode(ctx, ShlibInterfacesMode("defined_only"))
                        or cxx_use_shlib_intfs_mode(ctx, ShlibInterfacesMode("stub_from_library"))
                    ):
                        shared_lib_for_linking = shared_library_interface(
                            ctx = ctx,
                            shared_lib = shared_lib.output,
                        )
                    if ctx.attrs._target_os_type[OsLookup].os == Os("windows"):
                        shared_lib_for_linking = ctx.attrs.import_lib

                    linkable = None
                    if shared_lib_for_linking != None:
                        linkable = SharedLibLinkable(
                            lib = shared_lib_for_linking,
                            # Some prebuilt shared libs don't set a SONAME (e.g.
                            # IntelComposerXE), so we can't link them via just the shared
                            # lib (otherwise, we may embed build-time paths in `DT_NEEDED`
                            # tags).
                            link_without_soname = ctx.attrs.link_without_soname,
                        )

                    # Provided means something external to the build will provide
                    # the libraries, so we don't need to propagate anything.
                    if not ctx.attrs.provided and propagate_shared_libs:
                        solibs.append(
                            create_shlib_from_ctx(
                                ctx = ctx,
                                lib = shared_lib,
                                soname = soname,
                            ),
                        )

                    # Provide a sub-target that always provides the shared lib
                    # using the soname.
                    if soname and soname.is_str and shared_lib.output.basename != paths.basename(soname.ensure_str()):
                        soname_lib = ctx.actions.copy_file(soname.ensure_str(), shared_lib.output, has_content_based_path = False)
                    else:
                        soname_lib = shared_lib.output
                    sub_targets["soname-lib"] = [DefaultInfo(default_output = soname_lib)]

                    if shared_lib.pdb:
                        sub_targets[PDB_SUB_TARGET] = get_pdb_providers(pdb = shared_lib.pdb, binary = shared_lib.output)
                    dumpbin_toolchain_path = get_cxx_toolchain_info(ctx).dumpbin_toolchain_path
                    if dumpbin_toolchain_path != None:
                        sub_targets[DUMPBIN_SUB_TARGET] = get_dumpbin_providers(ctx, shared_lib.output, dumpbin_toolchain_path)
                    if shared_lib.dwp != None:
                        sub_targets["dwp"] = [DefaultInfo(default_output = shared_lib.dwp)]
                    if shared_lib.hip_arch_debug_files:
                        all_files = [f for files in shared_lib.hip_arch_debug_files.values() for f in files]
                        sub_targets["hip_debug"] = [
                            DefaultInfo(
                                default_outputs = all_files,
                                sub_targets = {arch: [DefaultInfo(default_outputs = files)] for arch, files in shared_lib.hip_arch_debug_files.items()},
                            ),
                        ]

        # TODO(cjhopman): is it okay that we sometimes don't have a linkable?
        outputs[output_style] = out
        dep_metadata = cxx_attr_dep_metadata(ctx)
        exported_flags, exported_post_flags = sandbox_exported_linker_flags(
            linker_info,
            linker_flags.exported_flags,
            linker_flags.exported_post_flags,
        )
        libraries[output_style] = LinkInfos(
            default = LinkInfo(
                name = ctx.attrs.name,
                pre_flags = exported_flags,
                post_flags = exported_post_flags,
                linkables = [linkable] if linkable else [],
                metadata = dep_metadata,
            ),
            stripped = None
            if linkable_stripped == None
            else LinkInfo(
                name = ctx.attrs.name,
                pre_flags = exported_flags,
                post_flags = exported_post_flags,
                linkables = [linkable_stripped],
                metadata = dep_metadata,
            ),
        )

        sub_targets[subtarget_for_output_style(output_style)] = [
            DefaultInfo(
                default_output = outputs[output_style],
            )
        ]

    # Third-party provider.
    shared_libs_for_third_party = SharedLibraries(libraries = solibs)
    if ctx.attrs.third_party_build != None:
        third_party_build_info = create_third_party_build_info(
            ctx = ctx,
            shared_libs = shared_libs_for_third_party.libraries if generated_shared_lib else [],
            children = [ctx.attrs.third_party_build[ThirdPartyBuildInfo]],
            deps = ctx.attrs.deps + cxx_attr_exported_deps(ctx),
        )
    else:
        third_party_build_info = create_third_party_build_info(
            ctx = ctx,
            paths = [] if header_dirs == None else [(d.short_path, d) for d in header_dirs],
            cxx_headers = [propagated_preprocessor],
            shared_libs = shared_libs_for_third_party.libraries,
            cxx_header_dirs = ["include"] + ([] if header_dirs == None else [d.short_path for d in header_dirs]),
            deps = ctx.attrs.deps + cxx_attr_exported_deps(ctx),
        )

    return struct(
        outputs = outputs,
        libraries = libraries,
        solibs = solibs,
        sub_targets = sub_targets,
        generated_shared_lib = generated_shared_lib,
        third_party_build_info = third_party_build_info,
    )

def _create_prebuilt_library_providers(
    ctx: AnalysisContext,
    input_providers: list[Provider],
    preferred_linkage: Linkage,
    library_outputs,
    static_lib,
    static_pic_lib,
    soname,
    linker_type,
    linker_flags,
    inherited_exported_link,
    inherited_link,
    first_order_deps,
    exported_first_order_deps,
):
    """
    Create and return providers for a prebuilt library with a specific preferred linkage.

    Takes the providers list constructed so far and library_outputs from _create_prebuilt_library_outputs,
    and appends link-related providers.
    Returns a tuple of (providers, sub_targets, default_output).
    """

    providers = list(input_providers)

    # Extract data from library_outputs for the output styles relevant to this preferred_linkage
    outputs = library_outputs.outputs
    libraries = {}
    for output_style in get_output_styles_for_linkage(preferred_linkage):
        libraries[output_style] = library_outputs.libraries[output_style]

    solibs = library_outputs.solibs
    sub_targets = dict(library_outputs.sub_targets)

    cxx_toolchain = get_cxx_toolchain_info(ctx)
    pic_behavior = cxx_toolchain.pic_behavior

    # Propagate link info provider.
    providers.append(
        create_merged_link_info(
            ctx,
            pic_behavior,
            # Add link info for each link style,
            libraries,
            preferred_linkage = preferred_linkage,
            # Export link info from non-exported deps (when necessary).
            deps = inherited_link,
            # Export link info from out (exported) deps.
            exported_deps = inherited_exported_link,
        )
    )

    # Propagate shared libraries up the tree.
    shared_libs = SharedLibraries(libraries = solibs)
    providers.append(
        merge_shared_libraries(
            ctx.actions,
            shared_libs,
            filter_and_map_idx(SharedLibraryInfo, exported_first_order_deps),
        )
    )

    providers.append(
        create_unix_env_info(
            actions = ctx.actions,
            env = UnixEnv(
                label = ctx.label,
                native_libs = [shared_libs],
            ),
            deps = ctx.attrs.deps + ctx.attrs.exported_deps,
        ),
    )

    # Third-party provider (already created in library_outputs)
    third_party_build_info = library_outputs.third_party_build_info
    providers.append(third_party_build_info)
    sub_targets["third-party-build"] = [
        DefaultInfo(
            default_output = third_party_build_info.build.root.artifact,
            sub_targets = dict(
                manifest = [DefaultInfo(default_output = third_party_build_info.build.manifest)],
            ),
        ),
    ]

    # Create the default output for the library rule given it's link style and preferred linkage
    link_strategy = to_link_strategy(cxx_toolchain.linker_info.link_style)
    actual_output_style = get_lib_output_style(link_strategy, preferred_linkage, pic_behavior)
    default_output = outputs[actual_output_style]

    # Omnibus root provider.
    if LibOutputStyle("pic_archive") in libraries and (static_pic_lib or static_lib) and not ctx.attrs.header_only and soname.is_str:
        # TODO(cjhopman): This doesn't support thin archives
        linkable_root = create_linkable_root(
            label = ctx.label,
            name = soname.ensure_str(),
            link_infos = LinkInfos(
                default = LinkInfo(
                    name = soname.ensure_str(),
                    pre_flags = (linker_flags.exported_flags + linker_flags.flags),
                    linkables = [
                        ArchiveLinkable(
                            archive = Archive(
                                artifact = static_pic_lib or static_lib,
                            ),
                            linker_type = linker_type,
                            link_whole = True,
                        )
                    ],
                    post_flags = linker_flags.exported_post_flags,
                    metadata = cxx_attr_dep_metadata(ctx),
                )
            ),
            deps = exported_first_order_deps,
        )
        providers.append(linkable_root)

        # Mark libraries that support `dlopen`.
        if ctx.attrs.supports_python_dlopen:
            if ctx.attrs.shared_lib != None and LibOutputStyle("shared_lib") in libraries:
                providers.append(DlopenableLibraryInfo(prebuilt_shared_lib = True))
            else:
                providers.append(DLOPENABLE_LIBRARY_INFO_MARKER)

    linkable_graph = create_linkable_graph(
        ctx,
        node = create_linkable_graph_node(
            ctx,
            linkable_node = create_linkable_node(
                ctx = ctx,
                default_soname = soname.as_str(),
                preferred_linkage = preferred_linkage,
                default_link_strategy = to_link_strategy(cxx_toolchain.linker_info.link_style),
                exported_deps = exported_first_order_deps,
                # If we don't have link input for this link style, we pass in `None` so
                # that omnibus knows to avoid it.
                link_infos = libraries,
                shared_libs = shared_libs,
                linker_flags = linker_flags,
                can_be_asset = getattr(ctx.attrs, "can_be_asset", False) or False,
                stub = getattr(ctx.attrs, "stub", False),
            ),
            excluded = {ctx.label: None} if not value_or(ctx.attrs.supports_merged_linking, True) else {},
        ),
        deps = exported_first_order_deps,
    )

    providers.append(linkable_graph)

    providers.append(
        merge_link_group_lib_info(
            deps = first_order_deps + exported_first_order_deps,
        ),
    )

    # TODO(T107163344) this shouldn't be in prebuilt_cxx_library itself, use overlays to remove it.
    providers.append(merge_android_packageable_info(ctx.label, ctx.actions, first_order_deps + exported_first_order_deps))

    apple_resource_graph = create_resource_graph(
        ctx = ctx,
        labels = ctx.attrs.labels,
        deps = first_order_deps,
        exported_deps = exported_first_order_deps,
    )
    providers += [apple_resource_graph]

    return (providers, sub_targets, default_output)

def prebuilt_cxx_library_impl(ctx: AnalysisContext) -> list[Provider]:
    # Versioned params should be intercepted and converted away via the stub.
    expect(not ctx.attrs.versioned_exported_lang_preprocessor_flags)
    expect(not ctx.attrs.versioned_exported_preprocessor_flags)
    expect(not ctx.attrs.versioned_header_dirs)
    expect(not ctx.attrs.versioned_shared_lib)
    expect(not ctx.attrs.versioned_static_lib)
    expect(not ctx.attrs.versioned_static_pic_lib)

    if not cxx_platform_supported(ctx):
        return [DefaultInfo(default_output = None)]

    providers = []

    toolchain_info = get_cxx_toolchain_info(ctx)
    linker_info = toolchain_info.linker_info
    linker_type = linker_info.type

    # Parse library parameters.
    static_lib = _prebuilt_item(
        ctx,
        ctx.attrs.static_lib,
    )
    static_pic_lib = _prebuilt_item(
        ctx,
        ctx.attrs.static_pic_lib,
    )
    shared_lib = _prebuilt_item(
        ctx,
        ctx.attrs.shared_lib,
    )
    header_dirs = _prebuilt_item(
        ctx,
        ctx.attrs.header_dirs,
    )
    preferred_linkage = _prebuilt_linkage(ctx)

    # Prepare the stripped static lib.
    static_lib_stripped = None
    if not ctx.attrs.prestripped and static_lib != None:
        static_lib_stripped = strip_debug_info(
            ctx.actions, static_lib.short_path, static_lib, toolchain_info, has_content_based_path = cxx_attr_use_content_based_paths(ctx)
        )

    # Prepare the stripped static PIC lib.  If the static PIC lib is the same
    # artifact as the static lib, then just re-use the stripped static lib.
    static_pic_lib_stripped = None
    if not ctx.attrs.prestripped:
        if static_lib == static_pic_lib:
            static_pic_lib_stripped = static_lib_stripped
        elif static_pic_lib != None:
            static_pic_lib_stripped = strip_debug_info(
                ctx.actions, static_pic_lib.short_path, static_pic_lib, toolchain_info, has_content_based_path = cxx_attr_use_content_based_paths(ctx)
            )

    if ctx.attrs.soname != None:
        soname = get_shared_library_name_for_param(linker_info, ctx.attrs.soname)
    elif shared_lib != None and ctx.attrs.extract_soname:
        soname = extract_soname_from_shlib(
            actions = ctx.actions,
            name = "__soname__.txt",
            shared_lib = shared_lib,
        )
    else:
        soname = get_shared_library_name(linker_info, ctx.label.name, apply_default_prefix = True)
    soname = to_soname(soname)

    # Use ctx.attrs.deps instead of cxx_attr_deps, since prebuilt rules don't have platform_deps.
    first_order_deps = ctx.attrs.deps
    exported_first_order_deps = cxx_attr_exported_deps(ctx)

    # Exported preprocessor info.
    inherited_pp_infos = cxx_inherited_preprocessor_infos(exported_first_order_deps)
    generic_exported_pre = cxx_exported_preprocessor_info(ctx, cxx_get_regular_cxx_headers_layout(ctx), [])
    args = []
    compiler_type = get_cxx_toolchain_info(ctx).cxx_compiler_info.compiler_type
    if header_dirs != None:
        for x in header_dirs:
            args.append(format_system_include_arg(cmd_args(x), compiler_type))
    exported_items = [generic_exported_pre]
    if args:
        exported_items.append(CPreprocessor(args = CPreprocessorArgs(args = args, precompile_args = args)))
    propagated_preprocessor = cxx_merge_cpreprocessors(
        ctx.actions,
        exported_items,
        inherited_pp_infos,
    )
    providers.append(propagated_preprocessor)

    inherited_link = cxx_inherited_link_info(first_order_deps)
    inherited_exported_link = cxx_inherited_link_info(exported_first_order_deps)

    linker_flags = cxx_attr_linker_flags_all(ctx)

    # Generate outputs for all output styles once
    # Only propagate shared libs if the preferred linkage allows shared linking
    propagate_shared_libs = LibOutputStyle("shared_lib") in get_output_styles_for_linkage(preferred_linkage)
    library_outputs = _create_prebuilt_library_outputs(
        ctx,
        static_lib,
        static_pic_lib,
        shared_lib,
        static_lib_stripped,
        static_pic_lib_stripped,
        soname,
        linker_type,
        linker_flags,
        inherited_exported_link,
        header_dirs,
        propagated_preprocessor,
        propagate_shared_libs,
    )

    # Create link-related providers.
    linkage_providers = {}
    for linkage in Linkage:
        linkage_providers[linkage] = _create_prebuilt_library_providers(
            ctx,
            providers,
            linkage,
            library_outputs,
            static_lib,
            static_pic_lib,
            soname,
            linker_type,
            linker_flags,
            inherited_exported_link,
            inherited_link,
            first_order_deps,
            exported_first_order_deps,
        )

    providers, sub_targets, output = linkage_providers[preferred_linkage]
    for linkage in (Linkage("static"), Linkage("shared")):
        lproviders, lsub_targets, loutput = linkage_providers[linkage]
        lproviders_with_default = list(lproviders)
        lproviders_with_default.append(
            DefaultInfo(
                default_output = loutput,
                sub_targets = lsub_targets,
            )
        )
        sub_targets["prefer-{}".format(linkage.value)] = lproviders_with_default
    # Usually zero or one exported header map.
    exported_header_maps = generic_exported_pre.args.header_maps
    if len(exported_header_maps) == 1:
        sub_targets["exported-header-map"] = [DefaultInfo(default_output = exported_header_maps[0])]
    providers.append(
        DefaultInfo(
            default_output = output,
            sub_targets = sub_targets,
        )
    )

    providers.append(
        make_resource_info(
            gather_resources(
                label = ctx.label,
                deps = first_order_deps + exported_first_order_deps,
            ),
        ),
    )

    return providers

def cxx_precompiled_header_impl(ctx: AnalysisContext) -> list[Provider]:
    if ctx.attrs.compile_pch_file:
        ctx.attrs.srcs.append(ctx.attrs.src)
        return cxx_library_generate(ctx, "cxx_precompiled_header")

    inherited_pp_infos = cxx_inherited_preprocessor_infos(ctx.attrs.deps)
    inherited_link = cxx_inherited_link_info(ctx.attrs.deps)
    return [
        DefaultInfo(default_output = ctx.attrs.src),
        cxx_merge_cpreprocessors(ctx.actions, [], inherited_pp_infos),
        create_merged_link_info_for_propagation(ctx, inherited_link),
        CPrecompiledHeaderInfo(header = ctx.attrs.src, compiled = False),
    ]

def cxx_test_impl(ctx: AnalysisContext) -> list[Provider]:
    link_strategy = to_link_strategy(cxx_attr_link_style(ctx))
    link_group_info = get_link_group_info(
        ctx,
        filter_and_map_idx(LinkableGraph, cxx_attr_deps(ctx)),
        link_strategy,
    )

    # TODO(T110378115): have the runinfo contain the correct test running args
    headers_layout = cxx_get_regular_cxx_headers_layout(ctx)
    srcs = get_srcs_with_flags(ctx)
    params = CxxRuleConstructorParams(
        rule_type = "cxx_test",
        generate_sub_targets = CxxRuleSubTargetParams(xcode_data = xcode_data_enabled()),
        headers_layout = headers_layout,
        srcs = srcs,
        link_group_info = link_group_info,
        auto_link_group_specs = get_auto_link_group_specs(ctx, link_group_info),
        prefer_stripped_objects = ctx.attrs.prefer_stripped_objects,
        force_remote_link_if_capable = ctx.attrs.link_execution_preference == "remote",
        extra_link_roots = linkables(ctx.attrs.link_group_deps),
        compiler_flags = ctx.attrs.compiler_flags,
        cxx_flags = cxx_attr_flags(ctx),
        lang_compiler_flags = ctx.attrs.lang_compiler_flags,
        preprocessor_flags = ctx.attrs.preprocessor_flags,
        lang_preprocessor_flags = ctx.attrs.lang_preprocessor_flags,
        use_header_units = _get_use_header_units_mode(ctx.attrs.use_header_units),
        runtime_dependency_handling = RuntimeDependencyHandling(ctx.attrs.runtime_dependency_handling)
        if ctx.attrs.runtime_dependency_handling
        else get_cxx_toolchain_info(ctx).runtime_dependency_handling,
        error_handler = get_cxx_toolchain_info(ctx).cxx_error_handler,
        extra_dwp_flags = ctx.attrs.extra_dwp_flags,
        use_fbcc_rust_wrapper = cxx_attrs_use_fbcc_rust_wrapper(ctx.attrs),
        allow_cache_upload = cxx_attrs_get_allow_cache_upload(ctx.attrs, get_cxx_toolchain_info(ctx).cxx_compiler_info.allow_cache_upload),
        precompiled_header = ctx.attrs.precompiled_header,
        prefix_header = ctx.attrs.prefix_header,
        _cxx_toolchain = ctx.attrs._cxx_toolchain,
        use_content_based_paths = cxx_attr_use_content_based_paths(ctx),
        coverage_instrumentation_compiler_flags = ctx.attrs.coverage_instrumentation_compiler_flags,
        coverage_profile_list = ctx.attrs.coverage_profile_list[DefaultInfo].default_outputs[0] if ctx.attrs.coverage_profile_list else None,
        coverage_profile_list_srcs = get_coverage_profile_list_srcs(ctx.attrs.coverage_profile_list),
        separate_debug_info = ctx.attrs.separate_debug_info,
        cuda_compile_style = CudaCompileStyle(ctx.attrs.cuda_compile_style),
        link_preference = LinkPreference(ctx.attrs.link_preference),
    )
    output = cxx_executable(ctx, params, is_cxx_test = True)

    target_stats_providers = []
    if TARGET_STATS_ENABLED:
        target_stats_tools = get_cxx_toolchain_info(ctx).target_stats_tools
        if target_stats_tools != None:
            target_stats_srcs = {src.file.short_path: src.file for src in srcs}
            target_stats_srcs.update({
                # Named headers use the dict key; list headers retain their
                # package-relative path. This matches the collector and avoids
                # collisions between headers with the same basename.
                (header.name if header.named else header.artifact.short_path): header.artifact
                for header in cxx_attr_headers_list(ctx, ctx.attrs.headers, headers_layout)
            })
            target_stats_providers, target_stats_subtargets = target_stats_providers_and_subtargets(
                ctx,
                tools = target_stats_tools,
                srcs = target_stats_srcs,
                deps = cxx_attr_deps(ctx),
                cycle_mode = CycleMode("file"),
                module_name = ctx.label.name,
            )
            output.sub_targets.update(target_stats_subtargets)

    command = [cmd_args(output.binary, hidden = output.runtime_files)] + ctx.attrs.args

    test_env = dict(ctx.attrs.env)
    static_lister = test_env.get("STATIC_LIST_TESTS_BINARY")
    if static_lister != None:
        listing_binary = test_env.get("BUCK_BASE_BINARY", output.binary)
        static_listing_library = test_env.get("STATIC_LISTING_DYLIB_NAME")
        if static_listing_library != None:
            libraries = [shlib.lib.output for shlib in output.shared_libs if shlib.soname.is_str and static_listing_library == shlib.soname.ensure_str()]
            listing_binary = libraries[0] if len(libraries) == 1 else None
        if listing_binary != None:
            test_env["TPX_LIST_TESTS_COMMAND"] = cmd_args(static_lister, listing_binary)

    # Setup RE executors based on the `remote_execution` param.
    re_executors = get_re_executors_from_props(ctx)

    providers = [
        DefaultInfo(
            default_output = output.binary,
            other_outputs = output.runtime_files + (output.external_debug_info_artifacts if get_cxx_toolchain_info(ctx).materialize_external_debug_info else []),
            sub_targets = output.sub_targets,
        ),
        output.compilation_db,
        output.dist_info,
    ]
    if output.xcode_data:
        providers.append(output.xcode_data)
    providers.extend(
        inject_test_run_info(
            ctx,
            ExternalRunnerTestInfo(
                type = "gtest",
                command = command,
                env = test_env,
                labels = ctx.attrs.labels,
                contacts = ctx.attrs.contacts,
                default_executor = re_executors.default_executor,
                executor_overrides = re_executors.executor_overrides,
                # We implicitly make this test via the project root, instead of
                # the cell root (e.g. fbcode root). `network_access` is carried on
                # the executor config (see `get_re_executors_from_props`).
                run_from_project_root = ("buck2_run_from_project_root" in (ctx.attrs.labels or []) or re_executors.run_from_project_root),
                use_project_relative_paths = re_executors.use_project_relative_paths,
                supports_test_execution_caching = ctx.attrs.supports_test_execution_caching,
            ),
        )
    )
    if output.validation_specs:
        providers.append(ValidationInfo(validations = output.validation_specs))

    if get_cxx_toolchain_info(ctx).gcno_files and output.gcno_files:
        providers.append(GcnoFilesInfo(gcno_files = output.gcno_files))

    providers.extend(target_stats_providers)

    return providers

def _get_params_for_android_binary_cxx_library() -> (CxxRuleSubTargetParams, CxxRuleProviderParams):
    sub_target_params = CxxRuleSubTargetParams(
        argsfiles = False,
        compilation_database = False,
        headers = False,
        link_group_map = False,
        xcode_data = False,
        clang_traces = False,
        objects = False,
        bitcode_bundle = False,
    )
    provider_params = CxxRuleProviderParams(
        compilation_database = False,
        omnibus_root = False,
        preprocessor_for_tests = False,
        cxx_resources_as_apple_resources = False,
    )

    return sub_target_params, provider_params

cxx_binary = prelude_rule(
    name = "cxx_binary",
    impl = cxx_binary_impl,
    docs = """
        A cxx\\_binary() rule builds a native executable from the supplied set of C/C++ source files and
        dependencies. If C/C++ library dependencies are listed, the generated native executable will request
        and link against their static archives (which are \\*not\\* built using [PIC](http://en.wikipedia.org/wiki/Position-independent_code)).
    """,
    examples = """
        ```
        # A rule that builds a C/C++ native executable from a single .cpp file
        # its corresponding header, and a C/C++ library dependency.
        cxx_binary(
          name = 'echo',
          srcs = [
            'echo.cpp',
          ],
          headers = [
            'echo.h',
          ],
          deps = [
            ':util',
          ],
        )

        cxx_library(
          name = 'util',
          srcs = [
            'util.cpp',
          ],
          headers = [
            'util.h',
          ],
        )

        # To build without stripping:
        buck build :echo

        # To build with stripping debug symbols only:
        buck build :echo#strip-debug
        ```
    """,
    further = None,
    attrs = (
        cxx_binary_base_attrs()
        | {
            "build_info_as_shared_library": attrs.bool(default = True),
            "_gen_build_info": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
            "_generated_build_info_data": attrs.option(attrs.source(), default = None),
            "_generated_build_info_enabled": attrs.bool(default = False),
            "_generated_build_info_mode": attrs.enum(["full", "stable"], default = "stable"),
            "_generated_build_info_spec": BUILD_INFO_ATTR,
        }
    ),
    # @oss-disable[end= ]: cfg = constraint_overrides.transition,
    supports_incoming_transition = True, # @oss-enable
)

cxx_library = prelude_rule(
    name = "cxx_library",
    impl = cxx_library_impl,
    docs = """
        A `cxx_library()` rule specifies a set of C/C++ source files
        and also provides flags that specify how those files should be built.

        #### Building requires a specified top-level target

        Whether a Buck command builds the `cxx_library` is
        determined by the inclusion of a top-level target, such as
        a `cxx_binary()` or `android_binary()`, that
        transitively depends on the `cxx_library`. The set of
        targets specified to the Buck command (`buck build`, `buck run`, etc) must
        include one of these top-level targets in order for Buck to build
        the `cxx_library`. Note that you could specify the top-level target
        implicitly using a `build target pattern` or you could also specify
        the top-level target using a buckconfig `alias` defined in `.buckconfig`.

        *How* Buck builds the library also depends on the specified top-level target.
        For example, a C/C++ binary (`cxx_binary`) would require a static non-PIC build of the library,
        whereas an Android APK (`android_binary`) would require a shared PIC-enabled build.
        (PIC stands for position-independent code.)

        #### Dependencies of the cxx\\_library also require a top-level target

        Similarly, in order for Buck to build a target that
        the `cxx_library` depends on, such as a `cxx_genrule()`,
        you must specify in the Buck command a top-level target that depends on
        the `cxx_library`. For example, you could specify
        to `build` a `cxx_binary` that
        depends on the `cxx_library`. If you specify as
        your build target the `cxx_library` itself, the build targets
        that the `cxx_library` depends on *might not be built*.
    """,
    examples = """
        ```
        # A rule that includes a single .cpp file and its corresponding header and
        # also supplies an additional flag for compilation.
        cxx_library(
          name = 'fileutil',
          srcs = [
            'fileutil.cpp',
          ],
          exported_headers = [
            'fileutil.h',
          ],
          compiler_flags = [
            '-fno-omit-frame-pointer',
          ],
        )

        # A rule that defines explicit names for its headers
        cxx_library(
          name = 'mathutils',
          header_namespace = 'math',
          srcs = [
            'trig/src/cos.cpp',
            'trig/src/tan.cpp',
          ],
          exported_headers = {
            # These are included as <math/trig/cos.h> and <math/trig/tan.h>
            'trig/cos.h': 'trig/include/cos.h',
            'trig/tan.h': 'trig/include/tan.h',
          },
          compiler_flags = [
            '-fno-omit-frame-pointer',
          ],
        )
        ```
    """,
    further = None,
    attrs = cxx_library_base_attrs() | cxx_library_extra_attrs(),
)

cxx_precompiled_header = prelude_rule(
    name = "cxx_precompiled_header",
    impl = cxx_precompiled_header_impl,
    docs = """
        A `cxx_precompiled_header` rule specifies a single header file that can be
         precompiled and made available for use in other build rules such as
         a `cxx_library()` or a `cxx_binary()`.

         This header file is precompiled by the preprocessor on behalf of the
         C, C++, Objective-C, or Objective-C++ rule using it, via its `precompiled_header` parameter.
         Afterwards the precompiled header is applied during the rule's own compilation
         (often with an appreciable reduction in build time, the main benefit of PCH).

         This PCH is built once per combination of build flags which might affect the PCH's compatibility.
         For example, a distinct pre-compilation of the header occurs per combination of flags related to
         optimization, debug, architecture, and so on, used by rules which employ PCH.
         The flags used during the build of the dependent rule (that is, the "PCH-using rule")
         are in effect while building the PCH itself. Similarly, to the same end, the include paths used
         when building the PCH are applied to the dependent rule. For example, `deps` in the
         PCH rule are propagated back to the dependent rule, and the PCH's header search paths
         (e.g. `-I` or `-isystem` options) are prefixed onto the list of
         include paths for the dependent rule.
    """,
    examples = """
        The best way to see how the `cxx_precompiled_header()` rule works is with an
        example. Let there be a header called `common.h` which has the following:

        ```
        #pragma once

        /* Include common C++ files. */
        #include <string>
        #include <map>
        #include <set>
        #include <type_traits>
        #include <vector>

        /* Some frequently-used headers from the Folly project. */
        #include <folly/Conv.h>
        #include <folly/Executor.h>
        #include <folly/io/async/EventBase.h>
        ```

        ```
        cxx_precompiled_header(
          name = 'common_pch',
          src = 'common.h',
          deps = [
            # Needed for standard C++ headers:
            '//external/libcxx:headers',
            # Needed for the Folly includes:
            '//folly:folly',
            '//folly/io/async:async',
          ],
        )

        cxx_binary(
          name = 'main',
          srcs = ['main.cpp'],
          precompiled_header = ':common_pch',
          deps = [ ... ],
          compiler_flags = ['-g', '-O2', '-fPIC'],
        )
        ```

        The `cxx_precompiled_header` rule declares a precompiled header "template"
        containing the header file path, and dependencies.
        In this example we indicate that `common.h` is to be precompiled when used by another build rule.

        Note that, by itself, this `cxx_precompiled_header` rule will not result
        in anything being built. The *usage* of this rule from another rule --
        an "instantiation" of this precompiled header template -- is what will trigger the
        PCH build.

        In the example above, the build for the binary named `"main"` will depend on
        the header being precompiled in a separate step, prior to compiling `main.cpp`,
        and the resulting PCH will be used in `main`'s compilation.

        The dependencies specified in this precompiled header rule's `deps` are transitive; they
        will propagate to rules using this PCH, so that during link time, any libraries which are
        required by the code made available in the header will be included in the final binary build.

        The precompiled header dynamically created from the "template" will be built with flags
        which would be used in the dependent rule. In this case, `main`'s use of specific
        compiler flags `-g -O2 -fPIC` will result in the production of a precompiled header
        with the same flags. This is so the precompiled code fully jives with rules using the PCH,
        i.e. they will have the same debug, optimization, CPU, etc. options. (The compiler is usually
        smart enough to reject a bad PCH, fortunately. But we want to ensure we take the appropriate
        steps to ensure we *always have* a PCH which works with any build that uses it.)

        Another effect of a rule using a precompiled header is that the rule's list of
        build flags will change; not just to employ PCH with e.g. `-include-pch` (if using Clang), but also, to alter the sequence of header search paths.
        The rule using the precompiled header will "inherit" the lists of paths used
        during the PCH build, applying them *first* in its own search paths.
        This is to ensure that an `#include` directive will resolve in exactly
        the same way in this build as it would have in the PCH, to ensure full compatibility
        between the PCH and other rule's builds. For example, if the PCH were to use one version
        of `stdcxx` and another rule use a different version, the version differences
        won't clash, thereby avoiding different versions of the `<cstring>` header
        used between the precompiled header and the dependent rule, and preventing confused
        structure definitions, ABI incompatibility, and so on (catastrophe, in other words).
    """,
    further = None,
    attrs = cxx_precompiled_header_attrs() | cxx_library_extra_attrs(),
)

CxxTestType = ["gtest", "boost"]

cxx_test = prelude_rule(
    name = "cxx_test",
    impl = cxx_test_impl,
    docs = """
        A cxx\\_test() rule builds a C/C++ binary against a C/C++ testing framework and runs
        it as part of `test`.
    """,
    examples = """
        ```
        # A rule that builds and runs C/C++ test using gtest.
        cxx_test(
          name = 'echo_test',
          srcs = [
            'echo_test.cpp',
          ],
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        buck.inject_test_env_arg()
        | cxx_common.srcs_arg()
        | cxx_common.headers_arg()
        | cxx_common.default_deps_arg()
        | cxx_common.preprocessor_flags_arg()
        | cxx_common.compiler_flags_arg()
        | cxx_common.linker_flags_arg()
        | cxx_common.precompiled_header_arg()
        | buck.deps_query_arg()
        | {
            "resources": attrs.named_set(
                attrs.source(),
                sorted = True,
                default = [],
                doc = """
                This attribute is currently not implemented, and just causes buck to rebuild
                 the test file if any of the resources change. This will change in the future
                 to provide a more reliable interface for resource files.

                 Additional data or source files which this test uses.
            """,
            ),
        }
        | cxx_common.raw_headers_arg()
        | cxx_common.raw_headers_as_headers_mode_arg()
        | cxx_common.include_directories_arg()
        | cxx_common.runtime_dependency_handling_arg()
        | cxx_common.use_fbcc_rust_wrapper_arg()
        | cxx_common.use_content_based_paths_arg()
        | {
            "args": attrs.list(
                attrs.arg(),
                default = [],
                doc = """
                A list of additional arguments to pass to the test when it's run.

                 It is also possible to expand references to other rules within these
                 arguments, using builtin `string parameter macros`
                :

                `$(location //path/to:target)`
                Expands to the location of the output of the build rule. This
                 means that you can refer to these without needing to be aware of how
                 Buck is storing data on the disk mid-build.
            """,
            ),
            "env": attrs.dict(
                key = attrs.string(),
                value = attrs.arg(),
                sorted = False,
                default = {},
                doc = """
                A map of environment names and values to set when running the test.

                 It is also possible to expand references to other rules within the **values** of
                 these environment variables, using builtin `string parameter macros`
                :

                `$(location //path/to:target)`
                Expands to the location of the output of the build rule. This
                 means that you can refer to these without needing to be aware of how
                 Buck is storing data on the disk mid-build.
            """,
            ),
            "framework": attrs.option(
                attrs.enum(CxxTestType),
                default = None,
                doc = """
                Unused.
            """,
            ),
        }
        | buck.run_test_separately_arg(run_test_separately_type = attrs.option(attrs.bool(), default = None))
        | buck.test_rule_timeout_ms()
        | native_common.link_group_deps()
        | native_common.link_group_public_deps_label()
        | native_common.transformation_spec_arg()
        | native_common.link_style()
        | {
            "additional_coverage_targets": attrs.list(attrs.source(), default = []),
            "cxx_runtime_type": attrs.option(attrs.enum(CxxRuntimeType), default = None),
            "default_platform": attrs.option(attrs.string(), default = None),
            "defaults": attrs.dict(key = attrs.string(), value = attrs.string(), sorted = False, default = {}),
            "deps": attrs.list(attrs.dep(), default = []),
            "devirt_enabled": attrs.bool(default = False),
            "executable_name": attrs.option(attrs.string(), default = None),
            "fat_lto": attrs.bool(default = False),
            "focused_list_target": attrs.option(attrs.dep(), default = None),
            "frameworks": attrs.list(attrs.string(), default = []),
            "header_namespace": attrs.option(attrs.string(), default = None),
            "headers_as_raw_headers_mode": attrs.option(attrs.enum(HeadersAsRawHeadersMode), default = None),
            "lang_compiler_flags": attrs.dict(key = attrs.enum(CxxSourceType), value = attrs.list(attrs.arg()), sorted = False, default = {}),
            "lang_preprocessor_flags": attrs.dict(key = attrs.enum(CxxSourceType), value = attrs.list(attrs.arg()), sorted = False, default = {}),
            "libraries": attrs.list(attrs.string(), default = []),
            "link_deps_query_whole": attrs.bool(default = False),
            "link_group": attrs.option(attrs.string(), default = None),
            "link_group_map": LINK_GROUP_MAP_ATTR,
            "linker_extra_outputs": attrs.list(attrs.string(), default = []),
            "post_linker_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
            "prefer_stripped_objects": attrs.bool(default = False),
            "prefix_header": attrs.option(attrs.source(), default = None),
            "supports_test_execution_caching": attrs.bool(default = True),
            "thin_lto": attrs.bool(default = False),
            "use_default_test_main": attrs.option(attrs.bool(), default = None),
            "use_header_units": attrs.one_of(
                attrs.bool(),
                attrs.enum(["stub"]),
                default = False,
                doc = """
                If True, makes any header unit exported by a dependency (including
                recursively) through export_header_unit available to the compiler. If
                false, the compilation ignores header units, regardless of what is
                exported by dependencies. If "stub", uses stub header units instead
                of full PCM files.
            """,
            ),
            "version_universe": attrs.option(attrs.string(), default = None),
            "weak_framework_names": attrs.list(attrs.string(), default = []),
        }
        | buck.allow_cache_upload_arg()
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | test_common.attributes()
        | cxx_binary_and_test_attrs()
        | re_test_common.test_args()
        | {
            "build_info_as_shared_library": attrs.bool(default = True),
            "_gen_build_info": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
            "_generated_build_info_data": attrs.option(attrs.source(), default = None),
            "_generated_build_info_enabled": attrs.bool(default = False),
            "_generated_build_info_mode": attrs.enum(["full", "stable"], default = "stable"),
            "_generated_build_info_spec": BUILD_INFO_ATTR,
        }
    ),
    # @oss-disable[end= ]: cfg = constraint_overrides.transition,
    supports_incoming_transition = True, # @oss-enable
)

prebuilt_cxx_library = prelude_rule(
    name = "prebuilt_cxx_library",
    impl = prebuilt_cxx_library_impl,
    docs = """
        A `prebuilt_cxx_library()` rule represents a set of native libraries and C/C++ header
        files and provides various flags to control how they are linked and exported.
    """,
    examples = """
        A prebuilt library containing only headers that other libraries may need.

        ```
        prebuilt_cxx_library(
          name = 'stdutil',
          header_only = True,
          header_dirs = [
            'include',
          ],
        )
        ```

         A prebuilt library with static and shared libs.

        ```
        prebuilt_cxx_library(
          name = 'mylib',
          soname = 'libmylib.so',
          static_lib = 'libmylib.a',
          static_pic_lib = 'libmylib_pic.a',
          shared_lib = 'libmylib.so',
          exported_headers = [
            'mylib.h',
          ],
        )
        ```
    """,
    further = None,
    attrs = (
        {
            "header_only": attrs.bool(
                default = False,
                doc = """
                Indicates if this library only consists of headers or not. If this is set to
                 `True`, Buck will not link this library into any library that depends on it.
            """,
            ),
            "shared_lib": attrs.option(
                attrs.source(),
                default = None,
                doc = """
                The path to the library to use when performing shared linking.
            """,
            ),
            "static_lib": attrs.option(
                attrs.source(),
                default = None,
                doc = """
                The path to the library to use when performing static linking.
            """,
            ),
            "static_pic_lib": attrs.option(
                attrs.source(),
                default = None,
                doc = """
                The path to the library to use when performing static PIC linking.
            """,
            ),
        }
        | cxx_common.supported_platforms_regex_arg()
        | cxx_common.exported_headers_arg()
        | cxx_common.header_namespace_arg()
        | cxx_common.exported_preprocessor_flags_arg(exported_preprocessor_flags_type = attrs.list(attrs.arg(), default = []))
        | cxx_common.exported_linker_flags_arg()
        | cxx_common.force_static(force_static_type = attrs.bool(default = False))
        | cxx_common.exported_deps_arg()
        | cxx_common.default_deps_arg("exported_deps")
        | cxx_common.supports_merged_linking()
        | cxx_common.local_linker_flags_arg()
        | cxx_common.local_linker_script_flags_arg()
        | cxx_common.version_arg()
        | {
            "can_be_asset": attrs.bool(default = False),
            "deffile": attrs.option(
                attrs.source(),
                default = None,
                doc = """
                Specifies the *.def file used on windows to modify a dll's exports in place of explicit `__declspec(dllexport)` declarations.
                 The default is to not use a defile.
            """,
            ),
            "deps": attrs.list(attrs.dep(), default = []),
            "exported_lang_preprocessor_flags": attrs.dict(key = attrs.enum(CxxSourceType), value = attrs.list(attrs.arg()), sorted = False, default = {}),
            "exported_post_linker_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
            "extract_soname": attrs.bool(default = False),
            "frameworks": attrs.list(attrs.string(), default = []),
            "import_lib": attrs.option(attrs.source(), default = None),
            "include_in_android_merge_map_output": attrs.bool(default = True),
            "libraries": attrs.list(attrs.string(), default = []),
            "link_whole": attrs.bool(default = False),
            "link_without_soname": attrs.bool(default = False),
            "platform_import_lib": attrs.option(attrs.list(attrs.tuple(attrs.regex(), attrs.source())), default = None),
            "prestripped": attrs.bool(default = False, doc = "When set, skips running `strip` commands when building this library."),
            "provided": attrs.bool(default = False),
            "soname": attrs.option(attrs.string(), default = None),
            "supports_shared_library_interface": attrs.bool(default = True),
            "versioned_exported_lang_preprocessor_flags": attrs.versioned(
                attrs.dict(key = attrs.enum(CxxSourceType), value = attrs.list(attrs.arg()), sorted = False)
            ),
            "versioned_exported_preprocessor_flags": attrs.versioned(attrs.list(attrs.arg())),
            "versioned_import_lib": attrs.option(attrs.versioned(attrs.source()), default = None),
            "versioned_shared_lib": attrs.option(attrs.versioned(attrs.source()), default = None),
            "versioned_soname": attrs.option(attrs.versioned(attrs.string()), default = None),
            "versioned_static_lib": attrs.option(attrs.versioned(attrs.source()), default = None),
            "versioned_static_pic_lib": attrs.option(attrs.versioned(attrs.source()), default = None),
        }
        | buck.allow_cache_upload_arg()
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | {
            "exported_header_style": attrs.enum(IncludeType, default = "system"),
            "header_dirs": attrs.option(
                attrs.list(attrs.source(allow_directory = True)),
                default = None,
                doc = """
                A directory that headers can be included from. These directories are added
                 to the include path using `-isystem`.
            """,
            ),
            "linker_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
            "post_linker_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
            "preferred_linkage": attrs.enum(
                Linkage.values(),
                default = "any",
                doc = """
        Determines what linkage is used when the library is depended on by another target. To
        control how the dependencies of this library are linked, use `link_style` instead.
        """,
            ),
            "public_include_directories": attrs.set(attrs.string(), sorted = True, default = []),
            "public_system_include_directories": attrs.set(attrs.string(), sorted = True, default = []),
            "raw_headers": attrs.set(attrs.source(), sorted = True, default = []),
            "stub": attrs.bool(default = False),
            "supports_lto": attrs.bool(default = False),
            "supports_python_dlopen": attrs.bool(default = True),
            "third_party_build": attrs.option(attrs.dep(providers = [ThirdPartyBuildInfo]), default = None),
            "versioned_header_dirs": attrs.option(attrs.versioned(attrs.list(attrs.source(allow_directory = True))), default = None),
            "_cxx_toolchain": toolchains_common.cxx(),
            "_target_os_type": buck.target_os_type_arg(),
        }
        | third_party_common.create_third_party_build_root_attrs()
    ),
)
