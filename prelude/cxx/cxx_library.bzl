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
)
load("@prelude//:attrs_validators.bzl", "get_attrs_validation_specs")
load("@prelude//:paths.bzl", "paths")
load(
    "@prelude//:resources.bzl",
    "gather_resources",
    "make_resource_info",
)
load(
    "@prelude//android:android_providers.bzl",
    "merge_android_packageable_info",
)
load(
    "@prelude//apple:apple_frameworks.bzl",
    "apple_build_link_args_with_deduped_flags",
    "apple_create_frameworks_linkable",
    "apple_get_link_info_by_deduping_link_infos",
    "get_framework_search_path_flags",
)
load(
    "@prelude//apple:apple_resource_types.bzl",
    "CxxResourceSpec",
)
load(
    "@prelude//apple:modularization_dependency_graph.bzl",
    "ModularizationDependencyGraphInfo",
    "create_modularization_dep_graph_subtargets_and_provider",
)
load("@prelude//apple:resource_groups.bzl", "create_resource_graph")
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo")
load(
    "@prelude//cxx:headers.bzl",
    "CPrecompiledHeaderInfo",
    "CxxPrecompiledHeader",
    "cxx_attr_precompiled_headers",
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
load(
    "@prelude//java:java_providers.bzl",
    "get_java_packaging_info",
    "propagate_global_code_info",
)
load("@prelude//linking:execution_preference.bzl", "LinkExecutionPreference", "get_link_execution_preference")
load(
    "@prelude//linking:link_groups.bzl",
    "EMPTY_LINK_GROUP_LIB_INFO",
    "LinkGroupLib",  # @unused Used as a type
    "gather_link_group_libs",
    "merge_link_group_lib_info",
)
load(
    "@prelude//linking:link_info.bzl",
    "ArchiveContentsType",
    "ArchiveLinkable",
    "FrameworksLinkable",  # @unused Used as a type
    "LibOutputStyle",
    "LinkArgs",
    "LinkCommandDebugOutputInfo",
    "LinkInfo",
    "LinkInfos",
    "LinkOrdering",
    "LinkStrategy",
    "LinkableFlavor",
    "LinkedObject",
    "ObjectsLinkable",
    "SharedLibLinkable",
    "SwiftmoduleLinkable",  # @unused Used as a type
    "UnstrippedLinkOutputInfo",
    "create_merged_link_info",
    "get_lib_output_style",
    "get_link_args_for_strategy",
    "get_output_styles_for_linkage",
    "make_link_command_debug_output",
    "make_link_command_debug_output_json_info",
    "process_link_strategy_for_pic_behavior",
    "subtarget_for_output_style",
    "to_link_strategy",
    "unpack_link_args",
    "wrap_link_info",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "DLOPENABLE_LIBRARY_INFO_MARKER",
    "LinkableRootInfo",
    "ReducedLinkableGraph",  # @unused used as a type
    "create_linkable_graph",
    "create_linkable_graph_node",
    "create_linkable_node",
    "linkable_deps",
    "reduce_linkable_graph",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "EMPTY_SHARED_LIBRARY_INFO",
    "NamedLinkedObject",
    "SharedLibraryInfo",
    "create_flavored_shared_libraries",
    "merge_shared_libraries",
)
load("@prelude//linking:strip.bzl", "strip_debug_info")
load("@prelude//linking:types.bzl", "Linkage")
load("@prelude//target_stats:target_stats.bzl", "CycleMode", "target_stats_providers_and_subtargets")
load("@prelude//target_stats:target_stats_config.bzl", "TARGET_STATS_ENABLED")
load(
    "@prelude//third-party:build.bzl",
    "create_third_party_build_info",
)
load("@prelude//unix:providers.bzl", "UnixEnv", "create_unix_env_info")
load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:lazy.bzl", "lazy")
load(
    "@prelude//utils:utils.bzl",
    "filter_and_map_idx",
    "flatten",
    "map_val",
    "value_or",
)
load("@prelude//xplugins:debug_artifacts.bzl", "xplugins_get_debug_artifacts_info")
load("@prelude//xplugins:utils.bzl", "get_xplugins_usage_info")
load(":archive.bzl", "make_archive")
load(
    ":argsfiles.bzl",
    "ARGSFILES_SUBTARGET",
    "get_argsfiles_output",
)
load(":bitcode.bzl", "BitcodeBundle", "BitcodeBundleInfo", "BitcodeTSet", "make_bitcode_bundle")
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
    "precompile_cxx",
    "toolchain_supports_flavor",
)
load(
    ":compile_types.bzl",
    "CxxCompileCommandOutput",
    "CxxCompileFlavor",
    "CxxCompileOutput",
    "CxxExtension",
    "CxxSrcCompileCommand",
    "CxxSrcPrecompileCommand",
)
load(":cxx_context.bzl", "get_cxx_platform_info", "get_cxx_toolchain_info")
load(
    ":cxx_instrumentation.bzl",
    "build_exported_needs_coverage",
    "needs_coverage",
)
load(
    ":cxx_library_utility.bzl",
    "EMPTY_DEFAULT_INFO",
    "OBJECTS_SUBTARGET",
    "cxx_attr_dep_metadata",
    "cxx_attr_deps",
    "cxx_attr_exported_deps",
    "cxx_attr_link_style",
    "cxx_attr_linker_flags_all",
    "cxx_attr_preferred_linkage",
    "cxx_attr_resources",
    "cxx_inherited_link_info",
    "cxx_is_gnu",
    "cxx_platform_supported",
    "cxx_use_shlib_intfs",
)
load(":cxx_sources.bzl", "CxxSrcWithFlags")
load(
    ":cxx_toolchain_types.bzl",
    "LinkerType",
    "ShlibInterfacesMode",
    "is_bitcode_format",
)
load(
    ":cxx_transitive_diagnostics.bzl",
    "cxx_transitive_diagnostics_combine",
)
load(
    ":cxx_types.bzl",
    "CxxLibraryInfo",
    "CxxRuleConstructorParams",
)
load(":diagnostics.bzl", "check_sub_target")
load(":gcno.bzl", "GcnoFilesInfo")
load(":hip_debug_extract.bzl", "PRE_EXTRACT_SUFFIX", "hip_debug_extract_available")
load(":index_store.bzl", "IndexStoreInfo", "create_index_store_subtargets_and_provider")
load(
    ":link.bzl",
    "CxxLinkResult",
    "CxxLinkerMapData",
    "cxx_link_shared_library",
    "link_args_have_hip_device_debug",
)
load(
    ":link_groups.bzl",
    "BuildLinkGroupsContext",
    "LINK_GROUPS_INFO_SUB_TARGET",
    "LINK_GROUP_MAP_DATABASE_SUB_TARGET",
    "collect_linkables",
    "get_filtered_labels_to_links_map",
    "get_filtered_links",
    "get_filtered_targets",
    "get_link_group",
    "get_link_group_info",
    "get_link_group_map_json",
    "get_link_group_preferred_linkage",
    "get_link_groups_info_json",
)
load(
    ":link_types.bzl",
    "link_options",
)
load(
    ":linker.bzl",
    "get_default_shared_library_name",
    "get_ignore_undefined_symbols_flags",
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
    "CPreprocessorForTestsInfo",
    "CPreprocessorInfo",
    "cxx_exported_preprocessor_info",
    "cxx_inherited_preprocessor_infos",
    "cxx_merge_cpreprocessors",
    "cxx_private_preprocessor_info",
)

# Interface modes in which the library's own link action yields the interface.
_INTERFACE_FROM_LIBRARY_LINK_MODES = [
    ShlibInterfacesMode("stub_from_linker_invocation"),
    ShlibInterfacesMode("stub_from_library"),
]

# A possible output of a `cxx_library`. This could be an archive or a shared library. Generally for an archive
# it represents just the sources of the library target itself, while a shared library will bundle multiple libraries
# together.
CxxLibraryOutput = record(
    # The output style of this output.
    output_style = field(LibOutputStyle),
    # The main output.
    default = field(Artifact),
    unstripped = field(Artifact),
    # Additional outputs that are implicitly used along with the above output
    # (e.g. external object files referenced by a thin archive).
    #
    # Note: It's possible that this can contain some of the artifacts which are
    # also present in object_files.
    other = field(list[Artifact], []),
    # The bitcode bundle. This only available for archive outputs.
    # TODO(cjhopman): always available? when is it/is it not available?
    bitcode_bundle = field([BitcodeBundle, None], None),
    # Additional debug info which referenced by but not included in the library output.
    external_debug_info = field(ArtifactTSet, ArtifactTSet()),
    # A shared shared library may have an associated dwp file with
    # its corresponding DWARF debug info.
    # May be None when Split DWARF is disabled, for static/static-pic libraries,
    # for some types of synthetic link objects or for pre-built shared libraries.
    dwp = field(Artifact | None, None),
    hip_arch_debug_files = field(dict[str, list[Artifact]], {}),
    # A shared shared library may have an associated PDB file with
    # its corresponding Windows debug info.
    pdb = field(Artifact | None, None),
    # The import library is the linkable output of a Windows shared library build.
    implib = field(Artifact | None, None),
    # Data about the linker map, only available on shared libraries
    # TODO(cjhopman): always available? when is it/is it not available?
    linker_map = field([CxxLinkerMapData, None], None),
    # Extra sub targets to be returned as outputs of this rule, by link style.
    sub_targets = field(dict[str, list[DefaultInfo]]),
    # The object files used to create the artifact in `default`. This only includes the object files
    # for this library itself, so it's not particularly meaningful for a shared lib output.
    object_files = field(list[Artifact], []),
)

# The outputs of either archiving or linking the outputs of the library
_CxxAllLibraryOutputs = record(
    # The outputs for each lib output style.
    # For 'static'/'static_pic', these will be archives containing just this library's object files.
    # For 'shared', the output as a shared library. That output will be built using either this library's link_strategy link strategy
    # or via the link group strategy if this library has link group mapping.
    # A header-only lib won't produce any outputs (but it may still provide LinkInfos below).
    # TODO(cjhopman): make library-level link_group shared lib not put its output here
    outputs = field(dict[LibOutputStyle, dict[LinkableFlavor, CxxLibraryOutput]]),
    # The link infos for linking against this lib of each output style. It's possible for a library to
    # add link_infos even when it doesn't produce an output itself.
    link_infos = field(dict[LibOutputStyle, LinkInfos]),
    # Extra providers to be returned consumers of this rule.
    providers = field(list[Provider], default = []),
    # Shared object name to shared library mapping if this target produces a shared library.
    solibs = field(dict[LinkableFlavor, NamedLinkedObject]),
    sanitizer_runtime_files = field(list[Artifact], []),
)

_CxxLibraryCompileOutput = record(
    # object files
    objects = field(list[Artifact]),
    # linkable object files from pch compilation
    pch_object_output = field(list[Artifact]),
    # object files stripped of debug information
    stripped_objects = field(list[Artifact]),
    # Those outputs which are bitcode
    bitcode_objects = field([list[Artifact], None]),
    # yaml file with optimization remarks about clang compilation
    clang_remarks = field([list[Artifact], None]),
    # llvm statistics about clang compilation
    clang_llvm_statistics = field([list[Artifact], None]),
    gcno_files = field([list[Artifact], None]),
    # json file with trace information about clang compilation
    clang_traces = field(list[Artifact]),
    # Externally referenced debug info, which doesn't get linked with the
    # object (e.g. the above `.o` when using `-gsplit-dwarf=single` or the
    # the `.dwo` when using `-gsplit-dwarf=split`).
    external_debug_info = field(list[Artifact]),
    # Whether there is any debug info
    objects_have_external_debug_info = field(bool),
    # sub_target for each object
    objects_sub_targets = field(dict[str, list[DefaultInfo]]),
    # the generated index stores
    index_stores = field(list[Artifact]),
    # diagnostics produced by a typecheck-only build (-fsyntax-only)
    diagnostics = field(dict[str, Artifact]),
    # diagnostics produced by clang-tidy
    clang_tidy_diagnostics = field(dict[str, Artifact]),
    has_hip_device_debug = field(bool, False),
    hip_arch_debug_files = field(dict[str, list[Artifact]], {}),
)

# The output of compiling all the source files in the library, containing
# the commands use to compile them and all the object file variants.
_CxxCompiledSourcesOutput = record(
    # Compile commands used to compile the source files or generate object files
    compile_cmds = field(CxxCompileCommandOutput),
    # PIC compile outputs
    pic = field(_CxxLibraryCompileOutput),
    # PIC optimized compile outputs
    pic_optimized = field([_CxxLibraryCompileOutput, None]),
    # PIC debuggable compile outputs
    pic_debuggable = field([_CxxLibraryCompileOutput, None]),
    # Non PIC compile outputs
    non_pic = field([_CxxLibraryCompileOutput, None]),
    # Header unit outputs
    header_unit_preprocessors = field(list[CPreprocessor]),
    precompile_cmds = field(list[CxxSrcPrecompileCommand]),
)

# The outputs of a cxx_library_parameterized rule.
_CxxLibraryParameterizedOutput = record(
    # The default output of a cxx library rule
    default_output = field([CxxLibraryOutput, None], None),
    # The other outputs available
    all_outputs = field([_CxxAllLibraryOutputs, None], None),
    # Any generated sub targets as requested by impl_params. Most of these will just be a
    # DefaultInfo to expose outputs to be consumed or built individually, but some (like "headers")
    # will have richer providers.
    sub_targets = field(dict[str, list[Provider]]),
    # A bundle of all bitcode files as a subtarget
    bitcode_bundle = field([BitcodeBundle, None], None),
    # Any generated providers as requested by impl_params.
    providers = field(list[Provider]),
    # XcodeDataInfo provider, returned separately as we cannot check
    # provider type from providers above
    xcode_data_info = field([XcodeDataInfo, None], None),
    # IndexStoreInfo provider, so we can access the paths of all the index stores
    # or Swift-specific index stores.
    index_store_info = field([IndexStoreInfo, None], None),
    # ModularizationDependencyGraphInfo provider for transitive modularization graphs.
    modularization_dep_graph_info = field([ModularizationDependencyGraphInfo, None], None),
    # CxxCompilationDbInfo provider, returned separately as we cannot check
    # provider type from providers above
    cxx_compilationdb_info = field([CxxCompilationDbInfo, None], None),
    # LinkableRootInfo provider, same as above.
    linkable_root = field([LinkableRootInfo, None], None),
    # List of shared libraries for the sanitizer runtime linked into the library
    sanitizer_runtime_files = field(list[Artifact], []),
)

def valid_pch_clanguage(val: typing.Any) -> bool:
    if not val:
        return False
    return val in [ext.value for ext in CxxExtension]

# FIXME(JakobDegen): This is used in place of `cxx_attr_link_style` in some places in a way that
# makes no sense to me and is not really in keeping with the rest of this API. But we rely on it.
# If you can justify it, replace with a comment.
def _link_strategy_for_some_shared_links(attrs: typing.Any) -> LinkStrategy:
    value = attrs.link_style if attrs.link_style != None else "shared"
    if value == "static":
        value = "static_pic"
    return LinkStrategy(value)

def cxx_library_parameterized(ctx: AnalysisContext, impl_params: CxxRuleConstructorParams) -> _CxxLibraryParameterizedOutput:
    """
    Defines the outputs for a cxx library, return the default output and any subtargets and providers based upon the requested params.
    """
    if not cxx_platform_supported(ctx):
        sub_targets = {}

        # Needed to handle cases of the named output (e.g. [static-pic]) being called directly.
        for output_style in get_output_styles_for_linkage(cxx_attr_preferred_linkage(ctx)):
            sub_targets[subtarget_for_output_style(output_style)] = [DefaultInfo(default_output = None)]

        return _CxxLibraryParameterizedOutput(
            providers = [
                DefaultInfo(default_output = None, sub_targets = sub_targets),
                EMPTY_SHARED_LIBRARY_INFO,
            ],
            sub_targets = sub_targets,
        )

    non_exported_deps = cxx_attr_deps(ctx)
    exported_deps = cxx_attr_exported_deps(ctx)
    deps_all_exported_first = exported_deps + non_exported_deps
    deps_all_non_exported_first = non_exported_deps + exported_deps

    compile_pch = None
    pch_clanguage = None
    if impl_params.rule_type == "cxx_precompiled_header":
        if len(ctx.attrs.srcs) != 1:
            fail("cxx_precompiled_header must have exactly one source file, got: {}".format(len(ctx.attrs.srcs)))

        pch_clanguage = ctx.attrs.pch_clanguage
        if not valid_pch_clanguage(pch_clanguage):
            fail("pch_clanguage was: {}, must be one of: {}".format(pch_clanguage, [ext.value for ext in CxxExtension]))

        pch_artifact = ctx.attrs.srcs[0]
        pch_header = cxx_attr_precompiled_headers(ctx, impl_params.headers_layout)
        basename = "{}{}".format(pch_artifact.basename, pch_clanguage)
        path = paths.join(pch_header.namespace, pch_header.name)

        pch_wrapper = ctx.actions.declare_output("pch_wrapper.cpp", has_content_based_path = False)
        ctx.actions.write(pch_wrapper, '#include "{}"'.format(path))

        ctx.attrs.headers.append(pch_artifact)
        impl_params.srcs[0] = CxxSrcWithFlags(file = pch_wrapper)

        compile_pch = CxxPrecompiledHeader(
            header = ctx.attrs.srcs[0],
            basename = basename,
            basename_src = pch_artifact.basename,
            path = path,
            namespace = pch_header.namespace,
            clanguage = pch_clanguage,
        )

    # TODO(T110378095) right now we implement reexport of exported_* flags manually, we should improve/automate that in the macro layer

    # Gather preprocessor inputs.
    (own_non_exported_preprocessor_info, test_preprocessor_infos) = cxx_private_preprocessor_info(
        ctx = ctx,
        raw_headers = ctx.attrs.raw_headers,
        headers_layout = impl_params.headers_layout,
        extra_preprocessors = impl_params.extra_preprocessors,
        non_exported_deps = non_exported_deps,
        is_test = impl_params.is_test,
    )
    own_exported_preprocessor_info = cxx_exported_preprocessor_info(
        ctx,
        impl_params.headers_layout,
        extra_preprocessors = impl_params.extra_exported_preprocessors,
    )
    own_preprocessors = [own_non_exported_preprocessor_info, own_exported_preprocessor_info] + test_preprocessor_infos
    own_exported_preprocessors = [own_exported_preprocessor_info]

    # Add framework search paths to exported preprocessors if frameworks attribute is set.
    # This is needed for the apple_library -> cxx_library swap in fb_xplat_cxx_library.
    frameworks = getattr(ctx.attrs, "frameworks", [])
    if frameworks:
        framework_search_paths_flags = get_framework_search_path_flags(ctx)
        framework_search_path_pre = CPreprocessor(
            args = CPreprocessorArgs(args = [framework_search_paths_flags]),
        )
        own_exported_preprocessors.insert(0, framework_search_path_pre)
        own_preprocessors.insert(0, framework_search_path_pre)

    inherited_non_exported_preprocessor_infos = cxx_inherited_preprocessor_infos(
        # Legacy precompiled_header implementation is not exported. Proper precompiled headers can have exported linkables.
        non_exported_deps
        + filter(
            None,
            [
                impl_params.precompiled_header
                if impl_params.precompiled_header and not impl_params.precompiled_header[CPrecompiledHeaderInfo].compiled
                else None
            ],
        ),
    )
    inherited_exported_preprocessor_infos = cxx_inherited_preprocessor_infos(exported_deps)

    preferred_linkage = cxx_attr_preferred_linkage(ctx)

    compilation_needs_coverage = build_exported_needs_coverage(ctx, deps_all_exported_first)
    compiled_srcs = cxx_compile_srcs(
        actions = ctx.actions,
        target_label = ctx.label,
        cxx_toolchain_info = get_cxx_toolchain_info(ctx),
        impl_params = impl_params,
        own_preprocessors = own_preprocessors,
        own_exported_preprocessors = own_exported_preprocessors,
        inherited_non_exported_preprocessor_infos = inherited_non_exported_preprocessor_infos,
        inherited_exported_preprocessor_infos = inherited_exported_preprocessor_infos,
        preferred_linkage = preferred_linkage,
        compile_pch = compile_pch,
        add_coverage_instrumentation_compiler_flags = needs_coverage(compilation_needs_coverage),
    )

    sub_targets = {}
    providers = []

    for name, preprocessor in (
        ("private-header-map", own_non_exported_preprocessor_info),
        ("exported-header-map", own_exported_preprocessor_info),
    ):
        for header_map in preprocessor.args.header_maps:
            sub_targets[name] = [DefaultInfo(default_output = header_map)]

    attr_validation_specs = get_attrs_validation_specs(ctx)
    if attr_validation_specs:
        providers.append(ValidationInfo(validations = attr_validation_specs))

    if compile_pch:
        pch_provider = CPrecompiledHeaderInfo(
            header = compile_pch.header,
            basename = compile_pch.basename,
            compiled = True,
            clanguage = pch_clanguage,
        )

        sub_targets["pch"] = [
            DefaultInfo(
                sub_targets = {
                    "default": [
                        CPrecompiledHeaderInfo(header = compiled_srcs.non_pic.objects[0], basename = compile_pch.basename, compiled = True, clanguage = pch_clanguage)
                    ]
                    if compiled_srcs.non_pic
                    else [],
                    CxxCompileFlavor("pic").value: [
                        CPrecompiledHeaderInfo(header = compiled_srcs.pic.objects[0], basename = compile_pch.basename, compiled = True, clanguage = pch_clanguage)
                    ]
                    if compiled_srcs.pic
                    else [],
                    CxxCompileFlavor("debug").value: [
                        CPrecompiledHeaderInfo(
                            header = compiled_srcs.pic_debuggable.objects[0], basename = compile_pch.basename, compiled = True, clanguage = pch_clanguage
                        )
                    ]
                    if compiled_srcs.pic_debuggable
                    else [],
                    CxxCompileFlavor("optimized").value: [
                        CPrecompiledHeaderInfo(
                            header = compiled_srcs.pic_optimized.objects[0], basename = compile_pch.basename, compiled = True, clanguage = pch_clanguage
                        )
                    ]
                    if compiled_srcs.pic_optimized
                    else [],
                },
            )
        ]

        providers.append(pch_provider)

    # Callers see exported headers; PCHs and reexport_all_header_dependencies can also expose private headers.
    if impl_params.coverage_profile_list_srcs and not compile_pch and not impl_params.precompiled_header and not _attr_reexport_all_header_dependencies(ctx):
        exported_needs_coverage = build_exported_needs_coverage(ctx, exported_deps)
    else:
        exported_needs_coverage = compilation_needs_coverage
    providers.append(exported_needs_coverage)

    if len(ctx.attrs.tests) > 0 and impl_params.generate_providers.preprocessor_for_tests:
        providers.append(
            CPreprocessorForTestsInfo(
                test_names = [test_target.name for test_target in ctx.attrs.tests],
                own_non_exported_preprocessor = own_non_exported_preprocessor_info,
            ),
        )

    if impl_params.generate_sub_targets.argsfiles:
        sub_targets[ARGSFILES_SUBTARGET] = [get_argsfiles_output(ctx, compiled_srcs.compile_cmds.argsfiles.relative, ARGSFILES_SUBTARGET)]
    if impl_params.generate_sub_targets.xcode_data:
        sub_targets[XCODE_ARGSFILES_SUB_TARGET] = [get_argsfiles_output(ctx, compiled_srcs.compile_cmds.argsfiles.xcode, XCODE_ARGSFILES_SUB_TARGET)]

    if impl_params.generate_sub_targets.clang_remarks:
        if compiled_srcs.non_pic and compiled_srcs.non_pic.clang_remarks:
            sub_targets["clang-remarks"] = [
                DefaultInfo(
                    default_outputs = compiled_srcs.non_pic.clang_remarks,
                )
            ]

        if compiled_srcs.pic.clang_remarks:
            sub_targets["pic-clang-remarks"] = [
                DefaultInfo(
                    default_outputs = compiled_srcs.pic.clang_remarks,
                )
            ]

    if impl_params.generate_sub_targets.clang_llvm_statistics:
        if compiled_srcs.non_pic and compiled_srcs.non_pic.clang_llvm_statistics:
            sub_targets["clang-llvm-statistics"] = [
                DefaultInfo(
                    default_outputs = compiled_srcs.non_pic.clang_llvm_statistics,
                )
            ]

        if compiled_srcs.pic.clang_llvm_statistics:
            sub_targets["pic-clang-llvm-statistics"] = [
                DefaultInfo(
                    default_outputs = compiled_srcs.pic.clang_llvm_statistics,
                )
            ]

    if impl_params.generate_sub_targets.clang_traces:
        traces = []
        if compiled_srcs.non_pic and compiled_srcs.non_pic.clang_traces:
            traces = compiled_srcs.non_pic.clang_traces
            sub_targets["clang-trace"] = [
                DefaultInfo(
                    default_outputs = traces,
                )
            ]

        clang_traces_tset = ctx.actions.tset(
            ClangTracesTSet,
            value = traces,
            children = [info.clang_traces for info in filter_and_map_idx(ClangTracesInfo, deps_all_exported_first)],
        )
        providers.append(ClangTracesInfo(clang_traces = clang_traces_tset))

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

        pic_traces = []
        if compiled_srcs.pic.clang_traces:
            pic_traces = compiled_srcs.pic.clang_traces
            sub_targets["pic-clang-trace"] = [
                DefaultInfo(
                    default_outputs = pic_traces,
                )
            ]

        pic_clang_traces_tset = ctx.actions.tset(
            ClangTracesTSet,
            value = pic_traces,
            children = [info.clang_traces for info in filter_and_map_idx(PicClangTracesInfo, deps_all_exported_first)],
        )
        providers.append(PicClangTracesInfo(clang_traces = pic_clang_traces_tset))

        pic_clang_traces_args = pic_clang_traces_tset.project_as_args("clang_traces")
        all_pic_traces = ctx.actions.write(
            ctx.actions.declare_output("recursive_pic_clang_traces.txt", has_content_based_path = False),
            pic_clang_traces_args,
        )

        sub_targets["pic-clang-traces"] = [
            DefaultInfo(
                default_output = all_pic_traces,
                other_outputs = [pic_clang_traces_args],
            )
        ]

    all_diagnostics = None

    # Collect source diagnostics; defer check_sub_target until after header-unit diagnostics
    input_diagnostics = {}
    input_clang_tidy_diagnostics = {}
    if impl_params.generate_sub_targets.objects:
        objects_sub_targets = compiled_srcs.pic.objects_sub_targets
        if compiled_srcs.non_pic:
            objects_sub_targets = objects_sub_targets | compiled_srcs.non_pic.objects_sub_targets
        sub_targets[OBJECTS_SUBTARGET] = [DefaultInfo(sub_targets = objects_sub_targets)]
        contains_extra_diagnostics = impl_params.extra_diagnostics and len(impl_params.extra_diagnostics) > 0
        if len(compiled_srcs.pic.diagnostics) > 0 or contains_extra_diagnostics or impl_params.export_header_unit:
            input_diagnostics = compiled_srcs.pic.diagnostics
            if contains_extra_diagnostics or impl_params.export_header_unit:
                # Avoid creation of merged dict unless needed, for efficiency reasons
                input_diagnostics = dict(input_diagnostics)
                if contains_extra_diagnostics:
                    input_diagnostics.update(impl_params.extra_diagnostics)
        input_clang_tidy_diagnostics = compiled_srcs.pic.clang_tidy_diagnostics

    # Compilation DB.
    if impl_params.generate_sub_targets.compilation_database:
        comp_db = create_compilation_database(ctx, compiled_srcs.compile_cmds.src_compile_cmds, "compilation-database")
        sub_targets["compilation-database"] = [comp_db]

        # Compilation DB including headers.
        comp_db = create_compilation_database(ctx, compiled_srcs.compile_cmds.comp_db_compile_cmds, "full-compilation-database")
        sub_targets["full-compilation-database"] = [comp_db]

    # comp_db_compile_cmds can include header files being compiled as C++ which should not be exposed in the [compilation-database] subtarget
    comp_db_info = None
    if impl_params.generate_providers.compilation_database:
        comp_db_info = make_compilation_db_info(
            src_compile_cmds = compiled_srcs.compile_cmds.comp_db_compile_cmds,
            precompile_cmds = compiled_srcs.precompile_cmds,
            toolchain_info = get_cxx_toolchain_info(ctx),
            platform_info = get_cxx_platform_info(ctx),
        )
        providers.append(comp_db_info)

    # Link Groups
    link_group = get_link_group(ctx)
    link_group_info = impl_params.link_group_info or get_link_group_info(ctx)

    if link_group_info:
        link_groups = link_group_info.groups
        link_group_mappings = link_group_info.mappings
        link_group_deps = [link_group_info.graph]
        link_group_libs = gather_link_group_libs(
            deps = deps_all_non_exported_first,
        )
        providers.append(link_group_info)
    else:
        link_groups = {}
        link_group_mappings = {}
        link_group_deps = []
        link_group_libs = {}
    link_group_preferred_linkage = get_link_group_preferred_linkage(link_groups.values())

    # Create the linkable graph from the library's deps, exported deps and any link group deps.
    linkable_graph_deps = deps_all_non_exported_first + link_group_deps
    deps_linkable_graph = create_linkable_graph(
        ctx,
        deps = linkable_graph_deps,
    )

    frameworks_linkable = apple_create_frameworks_linkable(ctx)
    swiftmodule_linkable = impl_params.swiftmodule_linkable
    shared_library_links = _get_shared_library_links(
        ctx,
        partial(reduce_linkable_graph, deps_linkable_graph),
        link_group,
        link_group_mappings,
        link_group_preferred_linkage,
        link_group_libs,
        exported_deps,
        non_exported_deps,
        impl_params.force_link_group_linking,
        frameworks_linkable,
        swiftmodule_linkable,
        force_static_follows_dependents = impl_params.link_groups_force_static_follows_dependents,
        link_groups_info_extras = impl_params.generate_sub_targets.xcode_data,
    )
    if impl_params.generate_sub_targets.link_group_map and shared_library_links.link_group_map_database:
        sub_targets[LINK_GROUP_MAP_DATABASE_SUB_TARGET] = [shared_library_links.link_group_map_database]
    if shared_library_links.link_groups_info:
        sub_targets[LINK_GROUPS_INFO_SUB_TARGET] = [shared_library_links.link_groups_info]

    extra_static_linkables = []
    if frameworks_linkable:
        extra_static_linkables.append(frameworks_linkable)
    if swiftmodule_linkable:
        extra_static_linkables.append(swiftmodule_linkable)

    library_outputs = _form_library_outputs(
        ctx = ctx,
        impl_params = impl_params,
        compiled_srcs = compiled_srcs,
        preferred_linkage = preferred_linkage,
        dep_infos = shared_library_links.link_args,
        extra_static_linkables = extra_static_linkables,
        gnu_use_link_groups = cxx_is_gnu(ctx) and bool(link_group_mappings),
        link_execution_preference = shared_library_links.link_execution_preference,
        compile_pch = compile_pch,
    )
    shared_libs = create_flavored_shared_libraries(ctx, library_outputs.solibs)

    for link_style_output in library_outputs.outputs.values():
        if LinkableFlavor("default") not in link_style_output:
            continue

        for key in link_style_output[LinkableFlavor("default")].sub_targets.keys():
            if key in sub_targets:
                fail("The subtarget `{}` already exists!".format(key))
        sub_targets.update(link_style_output[LinkableFlavor("default")].sub_targets)

    providers.extend(library_outputs.providers)

    pic_behavior = get_cxx_toolchain_info(ctx).pic_behavior

    # This is the output style for the library's own link_strategy+preferred_linkage.
    default_output_style = get_lib_output_style(to_link_strategy(cxx_attr_link_style(ctx)), preferred_linkage, pic_behavior)

    # Output sub-targets for all output-styles.
    if impl_params.generate_sub_targets.link_style_outputs or impl_params.generate_providers.link_style_outputs:
        default_output_style_providers = []
        for output_style in get_output_styles_for_linkage(preferred_linkage):
            output = library_outputs.outputs.get(output_style, {}).get(LinkableFlavor("default"), None)
            output_style_sub_targets, output_style_providers = impl_params.output_style_sub_targets_and_providers_factory(
                output_style,
                ctx,
                output,
            )
            if output != None and output.unstripped != None:
                output_style_providers.append(
                    UnstrippedLinkOutputInfo(artifact = output.unstripped),
                )

            if output:
                # Add any subtargets for this output style.
                output_style_sub_targets.update(output.sub_targets)

            if impl_params.generate_sub_targets.link_style_outputs:
                if output:
                    sub_targets[subtarget_for_output_style(output_style)] = [
                        DefaultInfo(
                            default_output = output.default,
                            other_outputs = output.other,
                            sub_targets = output_style_sub_targets,
                        )
                    ] + (output_style_providers if output_style_providers else [])

                if output_style == default_output_style:
                    # If we have additional providers for the current link style,
                    # add them to the list of default providers
                    # TODO(cjhopman): This looks like a bug, adding to providers probably shouldn't depend on generate_sub_target.link_style_outputs
                    default_output_style_providers += output_style_providers

                    # In addition, ensure any subtargets for the active link style
                    # can be accessed as a default subtarget
                    for output_style_sub_target_name, output_style_sub_target_providers in output_style_sub_targets.items():
                        sub_targets[output_style_sub_target_name] = output_style_sub_target_providers

        if impl_params.generate_providers.link_style_outputs:
            providers += default_output_style_providers

    # Create the default output for the library rule given it's link style and preferred linkage
    # It's possible for a library to not produce any output, for example, a header only library doesn't produce any archive or shared lib
    default_output = library_outputs.outputs.get(default_output_style, {}).get(LinkableFlavor("default"), None)

    if default_output and default_output.bitcode_bundle:
        sub_targets["bitcode"] = [DefaultInfo(default_output = default_output.bitcode_bundle.artifact)]

    # Define the xcode data sub target
    xcode_data_info = None
    if impl_params.generate_sub_targets.xcode_data:
        xcode_data_default_info, xcode_data_info = generate_xcode_data(
            ctx,
            rule_type = impl_params.rule_type,
            output = default_output.default if default_output else None,
            populate_rule_specific_attributes_func = impl_params.cxx_populate_xcode_attributes_func,
            srcs = impl_params.srcs + impl_params.additional.srcs,
            argsfiles = compiled_srcs.compile_cmds.argsfiles.xcode,
            product_name = get_default_cxx_library_product_name(ctx, impl_params),
        )
        sub_targets[XCODE_DATA_SUB_TARGET] = xcode_data_default_info
        providers.append(xcode_data_info)

    # Propagate link info provider.
    if impl_params.generate_providers.merged_native_link_info or impl_params.generate_providers.template_placeholders:
        # Gather link inputs.
        # Windows needs to include a linkable PCH object, this is not required for other platforms
        inherited_non_exported_link = cxx_inherited_link_info(non_exported_deps + filter(None, [impl_params.precompiled_header]))
        inherited_exported_link = cxx_inherited_link_info(exported_deps)

        merged_native_link_info = create_merged_link_info(
            ctx,
            pic_behavior,
            # Add link info for each link style,
            library_outputs.link_infos,
            preferred_linkage = preferred_linkage,
            # Export link info from non-exported deps (when necessary).
            deps = inherited_non_exported_link,
            # Export link info from out (exported) deps.
            exported_deps = inherited_exported_link,
            frameworks_linkable = frameworks_linkable,
            swiftmodule_linkable = swiftmodule_linkable,
        )
        if impl_params.generate_providers.merged_native_link_info:
            providers.append(merged_native_link_info)
    else:
        # This code sets merged_native_link_info only in some cases, leaving it unassigned in others.
        # Add a fake definition set to None so the assignment checker is satisfied.
        merged_native_link_info = None
        inherited_non_exported_link = None
        inherited_exported_link = None

    # Propagate shared libraries up the tree.
    if impl_params.generate_providers.shared_libraries:
        providers.append(
            merge_shared_libraries(
                ctx.actions,
                shared_libs,
                filter_and_map_idx(SharedLibraryInfo, deps_all_non_exported_first),
            )
        )
        providers.append(
            create_unix_env_info(
                actions = ctx.actions,
                env = UnixEnv(
                    label = ctx.label,
                    native_libs = [shared_libs],
                ),
                deps = deps_all_exported_first,
            ),
        )

    propagated_preprocessor_merge_list = inherited_exported_preprocessor_infos
    if _attr_reexport_all_header_dependencies(ctx):
        propagated_preprocessor_merge_list = inherited_non_exported_preprocessor_infos + propagated_preprocessor_merge_list

    # Header unit PCM.
    header_unit_diagnostics = {}
    if impl_params.generate_sub_targets.header_unit:
        if compiled_srcs.header_unit_preprocessors:
            header_unit_preprocessors = []
            header_unit_sub_targets = []
            header_unit_clang_traces = []

            def header_unit_clang_trace_sub_target(clang_traces):
                if impl_params.generate_sub_targets.clang_traces:
                    return {
                        "clang-trace": [
                            DefaultInfo(default_outputs = clang_traces),
                        ],
                    }
                else:
                    return {}

            def add_header_unit_clang_trace_sub_target(clang_traces):
                header_unit_clang_traces.extend(clang_traces)
                return header_unit_clang_trace_sub_target(clang_traces)

            def add_header_unit_check_sub_target(group_diagnostics, header_unit_group_idx):
                if group_diagnostics:
                    header_unit_diagnostics.update(group_diagnostics)
                    group_check_providers, _ = check_sub_target(
                        ctx,
                        group_diagnostics,
                        error_handler = impl_params.error_handler,
                        output_name = "header_unit{}.diagnostics.txt".format(header_unit_group_idx),
                    )
                    return {
                        "check": group_check_providers,
                    }
                else:
                    return {}

            header_unit_group_idx = 0
            for x in compiled_srcs.header_unit_preprocessors:
                header_unit_preprocessors.append(x)
                header_unit_group_idx += 1
                header_unit_sub_targets.append([
                    DefaultInfo(
                        default_outputs = [h.module for h in x.header_units],
                        sub_targets = (
                            add_header_unit_clang_trace_sub_target(filter(None, [h.clang_trace for h in x.header_units]))
                            | add_header_unit_check_sub_target(
                                {h.name: h.diagnostics for h in x.header_units if h.diagnostics != None}, header_unit_group_idx
                            )
                        ),
                    ),
                    cxx_merge_cpreprocessors(
                        ctx.actions,
                        own_exported_preprocessors + header_unit_preprocessors,
                        propagated_preprocessor_merge_list,
                    ),
                ])

            # Build [header-unit][check] from all header unit diagnostics
            header_unit_check_sub = {}
            if header_unit_diagnostics:
                header_unit_check_providers, _ = check_sub_target(
                    ctx,
                    header_unit_diagnostics,
                    error_handler = impl_params.error_handler,
                    output_name = "header_unit_diagnostics.txt",
                )
                header_unit_check_sub["check"] = header_unit_check_providers
            sub_targets["header-unit"] = [
                DefaultInfo(
                    default_outputs = [h.module for x in header_unit_preprocessors for h in x.header_units],
                    sub_targets = {str(i): x for i, x in enumerate(header_unit_sub_targets)}
                    | header_unit_clang_trace_sub_target(header_unit_clang_traces)
                    | header_unit_check_sub,
                ),
                header_unit_sub_targets[-1][1],
            ]
            if impl_params.export_header_unit:
                own_exported_preprocessors.extend(header_unit_preprocessors)
        else:
            sub_targets["header-unit"] = [
                EMPTY_DEFAULT_INFO,
                cxx_merge_cpreprocessors(
                    ctx.actions,
                    own_exported_preprocessors,
                    propagated_preprocessor_merge_list,
                ),
            ]

    # Build [check]: syntax diagnostics + header unit diagnostics in default output.
    # Clang-tidy diagnostics are accessible as per-file sub_targets only.
    if impl_params.export_header_unit:
        input_diagnostics.update(header_unit_diagnostics)
    if len(input_diagnostics) > 0:
        sub_targets["check"], all_diagnostics = check_sub_target(
            ctx,
            input_diagnostics,
            error_handler = impl_params.error_handler,
            extra_sub_targets = input_clang_tidy_diagnostics,
        )

    # Build [check-all]: syntax + clang-tidy + header unit diagnostics
    # (always includes header-unit diagnostics regardless of export_header_unit).
    check_all_diagnostics = dict(input_diagnostics)
    check_all_diagnostics.update(input_clang_tidy_diagnostics)
    if header_unit_diagnostics:
        check_all_diagnostics.update(header_unit_diagnostics)
    if len(check_all_diagnostics) > len(input_diagnostics):
        check_all_subtarget_result = check_sub_target(
            ctx,
            check_all_diagnostics,
            error_handler = impl_params.error_handler,
            output_name = "all_diagnostics.txt",
        )
        sub_targets["check-all"] = check_all_subtarget_result[0]

    propagated_preprocessor = cxx_merge_cpreprocessors(
        ctx.actions,
        own_exported_preprocessors,
        propagated_preprocessor_merge_list,
    )
    if impl_params.generate_providers.preprocessors:
        providers.append(propagated_preprocessor)

    # Propagated_exported_preprocessor_info is used for pcm compilation, which isn't possible for non-modular targets.
    propagated_exported_preprocessor_info = propagated_preprocessor if impl_params.rule_type == "apple_library" and ctx.attrs.modular else None
    additional_providers = (
        impl_params.additional.additional_providers_factory(propagated_exported_preprocessor_info)
        if impl_params.additional.additional_providers_factory
        else []
    )

    if impl_params.generate_providers.third_party_build:
        third_party_build_info = create_third_party_build_info(
            ctx = ctx,
            cxx_headers = [propagated_preprocessor],
            shared_libs = shared_libs.libraries,
            deps = deps_all_exported_first,
            project = ctx.attrs.third_party_project,
        )
        providers.append(third_party_build_info)
        sub_targets["third-party-build"] = [
            DefaultInfo(
                default_output = third_party_build_info.build.root.artifact,
                sub_targets = dict(
                    manifest = [DefaultInfo(default_output = third_party_build_info.build.manifest)],
                ),
            ),
        ]

    # For v1's `#headers` functionality.
    if impl_params.generate_sub_targets.headers:
        sub_targets["headers"] = [
            propagated_preprocessor,
            create_merged_link_info(
                ctx,
                pic_behavior = pic_behavior,
                preferred_linkage = Linkage("static"),
                frameworks_linkable = frameworks_linkable,
                swiftmodule_linkable = swiftmodule_linkable,
            ),
            EMPTY_LINK_GROUP_LIB_INFO,
            EMPTY_SHARED_LIBRARY_INFO,
        ] + additional_providers

    if getattr(ctx.attrs, "supports_header_symlink_subtarget", False):
        package_prefix = str(ctx.label.path)
        header_symlink_mapping = {}
        for records in propagated_preprocessor.set.traverse():
            for record in records:
                for header in record.headers:
                    header_path = header.name
                    if header.namespace:
                        header_path = paths.join(header.namespace, header_path)
                    header_symlink_mapping[paths.normalize(header_path)] = header.artifact

                all_include_dirs = record.include_dirs + (record.system_include_dirs.include_dirs if record.system_include_dirs else [])
                for header in record.raw_headers:
                    full_header_path = paths.join(package_prefix, header.short_path)
                    for include_dir in all_include_dirs:
                        include_dir_path = str(include_dir)

                        if not paths.starts_with(full_header_path, include_dir_path):
                            continue

                        mapping_path = paths.relativize(full_header_path, include_dir_path)
                        header_symlink_mapping[mapping_path] = header

        sub_targets["header-symlink-tree"] = [
            DefaultInfo(
                default_output = ctx.actions.symlinked_dir("header_symlink_tree", header_symlink_mapping, has_content_based_path = False),
            )
        ]

    for additional_subtarget, subtarget_providers in impl_params.additional.subtargets.items():
        sub_targets[additional_subtarget] = subtarget_providers

    index_stores = []
    swift_index_stores = []
    if impl_params.index_stores:
        # Index store from swift compile
        index_stores.extend(impl_params.index_stores)
        swift_index_stores.extend(impl_params.index_stores)

    # Index stores from cxx compile. We only generate the index store for pic
    if compiled_srcs.pic:
        index_stores.extend(compiled_srcs.pic.index_stores)

    index_store_subtargets, index_store_info = create_index_store_subtargets_and_provider(ctx, index_stores, swift_index_stores, deps_all_non_exported_first)
    sub_targets.update(index_store_subtargets)
    providers.append(index_store_info)

    mod_dep_graph_subtargets, mod_dep_graph_info = create_modularization_dep_graph_subtargets_and_provider(
        ctx,
        impl_params.modularization_dependency_graph,
        deps_all_non_exported_first,
    )
    sub_targets.update(mod_dep_graph_subtargets)
    providers.append(mod_dep_graph_info)

    linker_flags = cxx_attr_linker_flags_all(ctx, impl_params.cxx_flags)

    # Omnibus root provider.
    linkable_root = None
    if impl_params.generate_providers.omnibus_root:
        if impl_params.use_soname:
            soname = _soname(ctx, impl_params)
        else:
            soname = None
        linker_type = get_cxx_toolchain_info(ctx).linker_info.type
        linkable_root = create_linkable_root(
            label = ctx.label,
            name = soname,
            link_infos = LinkInfos(
                default = LinkInfo(
                    pre_flags = linker_flags.flags + linker_flags.exported_flags,
                    post_flags = linker_flags.post_flags + linker_flags.exported_post_flags,
                    linkables = [
                        ObjectsLinkable(
                            objects = compiled_srcs.pic.objects,
                            linker_type = linker_type,
                            link_whole = True,
                        )
                    ],
                    external_debug_info = make_artifact_tset(
                        actions = ctx.actions,
                        label = ctx.label,
                        artifacts = (
                            compiled_srcs.pic.external_debug_info + (compiled_srcs.pic.objects if compiled_srcs.pic.objects_have_external_debug_info else [])
                        ),
                        children = impl_params.additional.static_external_debug_info,
                    ),
                    has_hip_device_debug = compiled_srcs.pic.has_hip_device_debug,
                    hip_arch_debug_files = compiled_srcs.pic.hip_arch_debug_files,
                    metadata = cxx_attr_dep_metadata(ctx),
                ),
                stripped = LinkInfo(
                    pre_flags = linker_flags.flags + linker_flags.exported_flags,
                    post_flags = linker_flags.post_flags + linker_flags.exported_post_flags,
                    linkables = [
                        ObjectsLinkable(
                            objects = compiled_srcs.pic.stripped_objects,
                            linker_type = linker_type,
                            link_whole = True,
                        )
                    ],
                    has_hip_device_debug = compiled_srcs.pic.has_hip_device_debug,
                    hip_arch_debug_files = compiled_srcs.pic.hip_arch_debug_files,
                    metadata = cxx_attr_dep_metadata(ctx),
                ),
            ),
            deps = deps_all_non_exported_first,
        )
        providers.append(linkable_root)

        # Mark libraries that support `dlopen`.
        if getattr(ctx.attrs, "supports_python_dlopen", False):
            providers.append(DLOPENABLE_LIBRARY_INFO_MARKER)

    # Augment and provide the linkable graph.
    if impl_params.generate_providers.linkable_graph:
        merged_linkable_graph = create_linkable_graph(
            ctx,
            node = create_linkable_graph_node(
                ctx,
                linkable_node = create_linkable_node(
                    ctx = ctx,
                    default_soname = _soname(ctx, impl_params),
                    preferred_linkage = preferred_linkage,
                    default_link_strategy = _link_strategy_for_some_shared_links(ctx.attrs),
                    deps = non_exported_deps,
                    exported_deps = exported_deps,
                    # If we don't have link input for this link style, we pass in `None` so
                    # that omnibus knows to avoid it.
                    include_in_android_mergemap = getattr(ctx.attrs, "include_in_android_merge_map_output", True) and default_output != None,
                    # Attach this library's per-source compile commands so app rules can aggregate a
                    # native compile-command database. Gated (default off) to keep this off the graph
                    # for normal builds; see _emit_native_build_commands.
                    compile_cmds = compiled_srcs.compile_cmds.src_compile_cmds if getattr(ctx.attrs, "_emit_native_build_commands", False) else [],
                    link_infos = library_outputs.link_infos,
                    shared_libs = shared_libs,
                    linker_flags = linker_flags,
                    can_be_asset = getattr(ctx.attrs, "can_be_asset", False) or False,
                    stub = getattr(ctx.attrs, "stub", False),
                ),
                excluded = {ctx.label: None} if not value_or(ctx.attrs.supports_merged_linking, True) else {},
            ),
            deps = linkable_graph_deps,
        )
        providers.append(merged_linkable_graph)

    # C++ resource.
    if impl_params.generate_providers.resources:
        resources = cxx_attr_resources(ctx)
        providers.append(
            make_resource_info(
                gather_resources(
                    label = ctx.label,
                    resources = resources,
                    deps = deps_all_non_exported_first,
                ),
            ),
        )
        if impl_params.generate_providers.cxx_resources_as_apple_resources:
            apple_resource_graph = create_resource_graph(
                ctx = ctx,
                labels = ctx.attrs.labels,
                deps = non_exported_deps,
                exported_deps = exported_deps,
                cxx_resource_spec = CxxResourceSpec(resources = resources) if resources else None,
            )
            providers.append(apple_resource_graph)

    if impl_params.generate_providers.template_placeholders:
        templ_vars = {}

        # Some rules, e.g. fbcode//thrift/lib/cpp:thrift-core-module
        # define preprocessor flags as things like: -DTHRIFT_PLATFORM_CONFIG=<thrift/facebook/PlatformConfig.h>
        # and unless they get quoted, they break shell syntax.
        cxx_compiler_info = get_cxx_toolchain_info(ctx).cxx_compiler_info
        cxx_preprocessor_flags = cmd_args(
            cmd_args(cxx_compiler_info.preprocessor_flags or [], quote = "shell"),
            cmd_args(propagated_preprocessor.set.project_as_args("args"), quote = "shell"),
            propagated_preprocessor.set.project_as_args("include_dirs"),
        )
        templ_vars["cxxppflags"] = cxx_preprocessor_flags

        c_compiler_info = get_cxx_toolchain_info(ctx).c_compiler_info
        c_preprocessor_flags = cmd_args(
            cmd_args(c_compiler_info.preprocessor_flags or [], quote = "shell"),
            cmd_args(propagated_preprocessor.set.project_as_args("args"), quote = "shell"),
            propagated_preprocessor.set.project_as_args("include_dirs"),
        )
        templ_vars["cppflags"] = c_preprocessor_flags

        # Add in ldflag macros.
        for link_strategy in (LinkStrategy("static"), LinkStrategy("static_pic")):
            name = "ldflags-" + link_strategy.value.replace("_", "-")
            args = []
            linker_info = get_cxx_toolchain_info(ctx).linker_info
            args.append(linker_info.linker_flags or [])

            # Normally, we call get_link_args_for_strategy for getting the args for our own link from our
            # deps. This case is a bit different as we are effectively trying to get the args for how this library
            # would be represented on a dependent's link line and so it is appropriate to use our own merged_native_link_info.
            link_args = get_link_args_for_strategy(
                ctx.actions,
                ctx.label,
                get_cxx_toolchain_info(ctx).linker_info,
                [merged_native_link_info],
                link_strategy,
                prefer_stripped = False,
                transformation_spec_context = None,
            )
            args.append(unpack_link_args(link_args))
            templ_vars[name] = cmd_args(args)

        # TODO(T110378127): To implement `$(ldflags-shared ...)` properly, we'd need
        # to setup a symink tree rule for all transitive shared libs.  Since this
        # currently would be pretty costly (O(N^2)?), and since it's not that
        # commonly used anyway, just use `static-pic` instead.  Longer-term, once
        # v1 is gone, macros that use `$(ldflags-shared ...)` (e.g. Haskell's
        # hsc2hs) can move to a v2 rules-based API to avoid needing this macro.
        templ_vars["ldflags-shared"] = templ_vars["ldflags-static-pic"]

        providers.append(TemplatePlaceholderInfo(keyed_variables = templ_vars))

    # It is possible (e.g. in a java binary or an Android APK) to have C++ libraries that depend
    # upon Java libraries (through JNI). In some cases those Java libraries are not depended upon
    # anywhere else, so we need to expose them here to ensure that they are packaged into the
    # final binary.
    if impl_params.generate_providers.java_packaging_info:
        providers.append(get_java_packaging_info(ctx, deps_all_non_exported_first))

    if impl_params.generate_providers.java_global_code_info:
        providers.append(propagate_global_code_info(ctx, ctx.attrs.deps + getattr(ctx.attrs, "exported_deps", [])))

    # TODO(T107163344) this shouldn't be in cxx_library itself, use overlays to remove it.
    if impl_params.generate_providers.android_packageable_info:
        providers.append(merge_android_packageable_info(ctx.label, ctx.actions, deps_all_non_exported_first))

    bitcode_bundle = default_output.bitcode_bundle if default_output != None else None
    if bitcode_bundle:
        bc_provider = BitcodeBundleInfo(bitcode = bitcode_bundle, bitcode_bundle = ctx.actions.tset(BitcodeTSet, value = bitcode_bundle))
        additional_providers.append(bc_provider)

    # Add prefer-static and prefer-shared sub-targets for dependent-specific linkage control.
    # These expose the library with different preferred linkages, allowing dependents to
    # explicitly choose static or shared linking regardless of the library's default.
    if impl_params.generate_providers.merged_native_link_info:
        for linkage in (Linkage("static"), Linkage("shared")):
            # Get the appropriate output for this linkage
            linkage_output_style = get_lib_output_style(to_link_strategy(cxx_attr_link_style(ctx)), linkage, pic_behavior)
            linkage_output = library_outputs.outputs.get(linkage_output_style, {}).get(LinkableFlavor("default"), None)

            # Create MergedLinkInfo with this linkage (reuses inherited_*_link from above)
            linkage_merged_link_info = create_merged_link_info(
                ctx,
                pic_behavior,
                library_outputs.link_infos,
                preferred_linkage = linkage,
                deps = inherited_non_exported_link,
                exported_deps = inherited_exported_link,
                frameworks_linkable = frameworks_linkable,
                swiftmodule_linkable = swiftmodule_linkable,
            )

            # Check if we have the required output styles for this linkage
            required_output_styles = get_output_styles_for_linkage(linkage)
            has_required_outputs = True
            for style in required_output_styles:
                if style not in library_outputs.link_infos:
                    has_required_outputs = False
                    break

            # Include all providers needed for this sub-target to be usable as a dependency
            linkage_providers = [
                linkage_merged_link_info,
                propagated_preprocessor,
                EMPTY_LINK_GROUP_LIB_INFO,
                EMPTY_SHARED_LIBRARY_INFO,
            ]

            # Only create LinkableGraph if we have all required output styles for this linkage
            if has_required_outputs and impl_params.generate_providers.linkable_graph:
                linkage_linkable_graph = create_linkable_graph(
                    ctx,
                    node = create_linkable_graph_node(
                        ctx,
                        linkable_node = create_linkable_node(
                            ctx = ctx,
                            default_soname = _soname(ctx, impl_params),
                            preferred_linkage = linkage,
                            default_link_strategy = _link_strategy_for_some_shared_links(ctx.attrs),
                            deps = non_exported_deps,
                            exported_deps = exported_deps,
                            include_in_android_mergemap = getattr(ctx.attrs, "include_in_android_merge_map_output", True) and default_output != None,
                            compile_cmds = compiled_srcs.compile_cmds.src_compile_cmds if getattr(ctx.attrs, "_emit_native_build_commands", False) else [],
                            link_infos = library_outputs.link_infos,
                            shared_libs = shared_libs,
                            linker_flags = linker_flags,
                            can_be_asset = getattr(ctx.attrs, "can_be_asset", False) or False,
                            stub = getattr(ctx.attrs, "stub", False),
                        ),
                        excluded = {ctx.label: None} if not value_or(ctx.attrs.supports_merged_linking, True) else {},
                    ),
                    deps = linkable_graph_deps,
                )
                linkage_providers.append(linkage_linkable_graph)
            if linkage_output:
                linkage_providers.append(
                    DefaultInfo(
                        default_output = linkage_output.default,
                    )
                )
            sub_targets["prefer-{}".format(linkage.value)] = linkage_providers

    xplugins_usage_info = get_xplugins_usage_info(ctx.actions, deps_all_non_exported_first)
    if xplugins_usage_info:
        providers.append(xplugins_usage_info)

    xplugins_debug_info = xplugins_get_debug_artifacts_info(
        ctx,
        deps_all_non_exported_first,
    )
    if xplugins_debug_info:
        providers.append(xplugins_debug_info)

    if TARGET_STATS_ENABLED:
        target_stats_tools = get_cxx_toolchain_info(ctx).target_stats_tools
        if target_stats_tools != None:
            # Keyed by each file's path within the target, which is what a
            # consumer can line up against the target's own source list.
            target_stats_srcs = {src.file.short_path: src.file for src in impl_params.srcs + impl_params.additional.srcs}
            target_stats_srcs.update(impl_params.target_stats_extra_srcs)
            target_stats_providers, target_stats_subtargets = target_stats_providers_and_subtargets(
                ctx,
                tools = target_stats_tools,
                srcs = target_stats_srcs,
                deps = non_exported_deps + exported_deps,
                cycle_mode = CycleMode(impl_params.target_stats_cycle_mode),
                module_name = impl_params.target_stats_module_name or ctx.label.name,
                swift_dot = impl_params.target_stats_swift_dot,
            )
            providers.extend(target_stats_providers)
            sub_targets.update(target_stats_subtargets)

    if impl_params.generate_providers.default:
        if False:
            # TODO(nga): `default_output.unstripped` is never `None`.
            def unknown():
                pass

            default_output = unknown()

        default_info = DefaultInfo(
            default_output = default_output.default if default_output != None else None,
            other_outputs = default_output.other if default_output != None else [],
            sub_targets = sub_targets,
        )
        providers.append(default_info)

    # Propagate all transitive link group lib roots up the tree, so that the
    # final executable can use them.
    if impl_params.generate_providers.merged_native_link_info:
        providers.append(
            merge_link_group_lib_info(
                label = ctx.label,
                name = link_group,
                shared_libs = shared_libs,
                shared_link_infos = library_outputs.link_infos.get(LibOutputStyle("shared_lib")),
                deps = deps_all_exported_first,
            ),
        )

    if impl_params.generate_providers.transitive_diagnostics:
        providers.append(
            cxx_transitive_diagnostics_combine(
                ctx = ctx,
                diagnostics = filter(None, [all_diagnostics]) + impl_params.extra_transitive_diagnostics,
                deps = deps_all_exported_first,
            )
        )

    if getattr(ctx.attrs, "_meta_apple_library_validation_enabled", False):
        providers.append(
            CxxLibraryInfo(
                target = ctx.label,
                labels = ctx.attrs.labels,
            ),
        )

    providers.extend(additional_providers)

    return _CxxLibraryParameterizedOutput(
        default_output = default_output,
        all_outputs = library_outputs,
        sub_targets = sub_targets,
        bitcode_bundle = bitcode_bundle,
        providers = providers,
        index_store_info = index_store_info,
        modularization_dep_graph_info = mod_dep_graph_info,
        xcode_data_info = xcode_data_info,
        cxx_compilationdb_info = comp_db_info,
        linkable_root = linkable_root,
        sanitizer_runtime_files = library_outputs.sanitizer_runtime_files,
    )

def get_default_cxx_library_product_name(ctx, impl_params) -> str:
    preferred_linkage = cxx_attr_preferred_linkage(ctx)
    output_style = get_lib_output_style(
        to_link_strategy(cxx_attr_link_style(ctx)),
        preferred_linkage,
        get_cxx_toolchain_info(ctx).pic_behavior,
    )
    if output_style == LibOutputStyle("shared_lib"):
        return _soname(ctx, impl_params)
    else:
        return _base_static_library_name(ctx, optimized = False, debuggable = False, stripped = False)

def _collect_hip_arch_debug_files(outs: list[CxxCompileOutput]) -> dict[str, list[Artifact]]:
    result = {}
    for out in outs:
        for arch, f in out.hip_arch_debug_files.items():
            result.setdefault(arch, [])
            result[arch].append(f)
    return result

def _get_library_compile_output(
    actions: AnalysisActions,
    cxx_toolchain_info: CxxToolchainInfo,
    src_compile_cmds: list[CxxSrcCompileCommand],
    outs: list[CxxCompileOutput],
    extra_link_input: list[Artifact],
    supports_stripping: bool,
    has_content_based_path: bool,
) -> _CxxLibraryCompileOutput:
    objects = [out.object for out in outs]
    stripped_objects = _strip_objects(actions, cxx_toolchain_info, outs, supports_stripping, has_content_based_path)

    pch_object_output = [out.pch_object for out in outs if out.pch_object]

    bitcode_objects = [out.object for out in outs if is_bitcode_format(out.object_format)]
    if len(bitcode_objects) == 0:
        bitcode_objects = None

    objects_sub_targets = cxx_objects_sub_targets(outs)

    # add this after setting up sub_targets
    objects += extra_link_input
    stripped_objects += extra_link_input

    index_stores = [out.index_store for out in outs if out.index_store]

    diagnostics = {compile_cmd.src.short_path: out.diagnostics for compile_cmd, out in zip(src_compile_cmds, outs) if out.diagnostics != None}
    clang_tidy_diagnostics = {
        compile_cmd.src.short_path + ".clang-tidy": out.clang_tidy_diagnostics
        for compile_cmd, out in zip(src_compile_cmds, outs)
        if out.clang_tidy_diagnostics != None
    }

    return _CxxLibraryCompileOutput(
        objects = objects,
        pch_object_output = pch_object_output,
        stripped_objects = stripped_objects,
        bitcode_objects = bitcode_objects,
        clang_traces = [out.clang_trace for out in outs if out.clang_trace != None],
        clang_remarks = [out.clang_remarks for out in outs if out.clang_remarks != None],
        clang_llvm_statistics = [out.clang_llvm_statistics for out in outs if out.clang_llvm_statistics != None],
        gcno_files = [out.gcno_file for out in outs if out.gcno_file != None],
        external_debug_info = [out.external_debug_info for out in outs if out.external_debug_info != None],
        objects_have_external_debug_info = lazy.is_any(lambda out: out.object_has_external_debug_info, outs),
        has_hip_device_debug = lazy.is_any(lambda out: out.hip_device_debug, outs),
        hip_arch_debug_files = _collect_hip_arch_debug_files(outs),
        objects_sub_targets = objects_sub_targets,
        index_stores = index_stores,
        diagnostics = diagnostics,
        clang_tidy_diagnostics = clang_tidy_diagnostics,
    )

_CxxSharedLibraryResult = record(
    # Result from link, includes the shared lib, linker map data etc
    link_result = CxxLinkResult,
    # Shared library name (e.g. SONAME)
    soname = str,
    # `LinkInfo` used to link against the shared library.
    info = LinkInfo,
)

def cxx_compile_srcs(
    actions: AnalysisActions,
    target_label: Label,
    cxx_toolchain_info: CxxToolchainInfo,
    impl_params: CxxRuleConstructorParams,
    own_preprocessors: list[CPreprocessor],
    inherited_non_exported_preprocessor_infos: list[CPreprocessorInfo],
    inherited_exported_preprocessor_infos: list[CPreprocessorInfo],
    preferred_linkage: Linkage,
    add_coverage_instrumentation_compiler_flags: bool,
    compile_pch: CxxPrecompiledHeader | None = None,
    own_exported_preprocessors: list[CPreprocessor] = [],
    filename_prefix: str = "",
) -> _CxxCompiledSourcesOutput:
    """
    Compile objects we'll need for archives and shared libraries.
    """

    # Create the commands and argsfiles to use for compiling each source file
    compile_cmd_output = create_compile_cmds(
        actions = actions,
        target_label = target_label,
        toolchain = cxx_toolchain_info,
        impl_params = impl_params,
        own_preprocessors = own_preprocessors,
        inherited_preprocessor_infos = inherited_non_exported_preprocessor_infos + inherited_exported_preprocessor_infos,
        add_coverage_instrumentation_compiler_flags = add_coverage_instrumentation_compiler_flags,
        compile_pch = compile_pch,
        filename_prefix = filename_prefix,
    )

    # Define header unit.
    header_unit_preprocessors = []
    precompile_cmds = []
    if own_exported_preprocessors:
        header_preprocessor_info = cxx_merge_cpreprocessors(
            actions,
            own_exported_preprocessors,
            inherited_exported_preprocessor_infos,
        )
        header_unit_preprocessors, precompile_cmds = precompile_cxx(
            actions = actions,
            target_label = target_label,
            toolchain = cxx_toolchain_info,
            impl_params = impl_params,
            preprocessors = own_exported_preprocessors,
            header_preprocessor_info = header_preprocessor_info,
        )

    # Define object files.
    pic_cxx_outs = compile_cxx(
        actions = actions,
        target_label = target_label,
        toolchain = cxx_toolchain_info,
        src_compile_cmds = compile_cmd_output.src_compile_cmds,
        flavors = set([CxxCompileFlavor("pic")]),
        provide_syntax_only = True,
        separate_debug_info = impl_params.separate_debug_info,
        use_header_units = impl_params.use_header_units,
        precompiled_header = impl_params.precompiled_header,
        compile_pch = compile_pch,
        cuda_compile_style = impl_params.cuda_compile_style,
    )
    pic = _get_library_compile_output(
        actions = actions,
        cxx_toolchain_info = cxx_toolchain_info,
        src_compile_cmds = compile_cmd_output.src_compile_cmds,
        outs = pic_cxx_outs,
        extra_link_input = impl_params.extra_link_input,
        supports_stripping = impl_params.supports_stripping,
        has_content_based_path = impl_params.use_content_based_paths,
    )

    non_pic = None
    pic_optimized = None
    if preferred_linkage != Linkage("shared"):
        non_pic_cxx_outs = compile_cxx(
            actions = actions,
            target_label = target_label,
            toolchain = cxx_toolchain_info,
            src_compile_cmds = compile_cmd_output.src_compile_cmds,
            flavors = set(),
            # Diagnostics from the pic and non-pic compilation would be
            # identical. We can avoid instantiating a second set of actions.
            provide_syntax_only = False,
            separate_debug_info = impl_params.separate_debug_info,
            compile_pch = compile_pch,
            precompiled_header = impl_params.precompiled_header,
            use_header_units = impl_params.use_header_units,
            cuda_compile_style = impl_params.cuda_compile_style,
        )
        non_pic = _get_library_compile_output(
            actions = actions,
            cxx_toolchain_info = cxx_toolchain_info,
            src_compile_cmds = compile_cmd_output.src_compile_cmds,
            outs = non_pic_cxx_outs,
            extra_link_input = impl_params.extra_link_input,
            supports_stripping = impl_params.supports_stripping,
            has_content_based_path = impl_params.use_content_based_paths,
        )

        if toolchain_supports_flavor(cxx_toolchain_info, CxxCompileFlavor("optimized")):
            optimized_cxx_outs = compile_cxx(
                actions = actions,
                target_label = target_label,
                toolchain = cxx_toolchain_info,
                src_compile_cmds = compile_cmd_output.src_compile_cmds,
                flavors = set([CxxCompileFlavor("pic"), CxxCompileFlavor("optimized")]),
                # Diagnostics from the pic and non-pic compilation would be
                # identical. We can avoid instantiating a second set of actions.
                provide_syntax_only = False,
                separate_debug_info = impl_params.separate_debug_info,
                use_header_units = impl_params.use_header_units,
                cuda_compile_style = impl_params.cuda_compile_style,
            )
            pic_optimized = _get_library_compile_output(
                actions = actions,
                cxx_toolchain_info = cxx_toolchain_info,
                src_compile_cmds = compile_cmd_output.src_compile_cmds,
                outs = optimized_cxx_outs,
                extra_link_input = impl_params.extra_link_input,
                supports_stripping = impl_params.supports_stripping,
                has_content_based_path = impl_params.use_content_based_paths,
            )

    pic_debuggable = None
    if toolchain_supports_flavor(cxx_toolchain_info, CxxCompileFlavor("debug")):
        debuggable_cxx_outs = compile_cxx(
            actions = actions,
            target_label = target_label,
            toolchain = cxx_toolchain_info,
            src_compile_cmds = compile_cmd_output.src_compile_cmds,
            flavors = set([CxxCompileFlavor("pic"), CxxCompileFlavor("debug")]),
            # Diagnostics from the pic and non-pic compilation would be
            # identical. We can avoid instantiating a second set of actions.
            provide_syntax_only = False,
            separate_debug_info = impl_params.separate_debug_info,
            use_header_units = impl_params.use_header_units,
            cuda_compile_style = impl_params.cuda_compile_style,
        )
        pic_debuggable = _get_library_compile_output(
            actions = actions,
            cxx_toolchain_info = cxx_toolchain_info,
            src_compile_cmds = compile_cmd_output.src_compile_cmds,
            outs = debuggable_cxx_outs,
            extra_link_input = impl_params.extra_link_input,
            supports_stripping = impl_params.supports_stripping,
            has_content_based_path = impl_params.use_content_based_paths,
        )
    return _CxxCompiledSourcesOutput(
        compile_cmds = compile_cmd_output,
        pic = pic,
        pic_optimized = pic_optimized,
        pic_debuggable = pic_debuggable,
        non_pic = non_pic,
        header_unit_preprocessors = header_unit_preprocessors,
        precompile_cmds = precompile_cmds,
    )

def _form_library_outputs(
    ctx: AnalysisContext,
    impl_params: CxxRuleConstructorParams,
    compiled_srcs: _CxxCompiledSourcesOutput,
    preferred_linkage: Linkage,
    dep_infos: LinkArgs,
    extra_static_linkables: list[[FrameworksLinkable, SwiftmoduleLinkable]],
    gnu_use_link_groups: bool,
    link_execution_preference: LinkExecutionPreference,
    compile_pch: CxxPrecompiledHeader | None,
) -> _CxxAllLibraryOutputs:
    # Build static/shared libs and the link info we use to export them to dependents.
    outputs = {}
    solibs = {}
    link_infos = {}
    providers = []
    sanitizer_runtime_files = []
    gcno_files = []

    linker_info = get_cxx_toolchain_info(ctx).linker_info
    linker_flags = cxx_attr_linker_flags_all(ctx, impl_params.cxx_flags)

    # Add in exported linker flags.
    def ldflags(inner: LinkInfo) -> LinkInfo:
        exported_flags, exported_post_flags = sandbox_exported_linker_flags(
            linker_info,
            linker_flags.exported_flags,
            linker_flags.exported_post_flags,
        )
        return wrap_link_info(
            inner = inner,
            pre_flags = exported_flags,
            post_flags = exported_post_flags,
        )

    def build_static_library(
        compile_output: _CxxLibraryCompileOutput | None, pic: bool, optimized: bool, debuggable: bool, pch: bool, stripped: bool
    ) -> (CxxLibraryOutput | None, LinkInfo | None):
        if not compile_output:
            return (None, None)

        objects = compile_output.pch_object_output if pch else compile_output.stripped_objects if stripped else compile_output.objects

        if not objects:
            return (None, None)

        return _static_library(
            ctx,
            impl_params,
            objects,
            objects_have_external_debug_info = compile_output.objects_have_external_debug_info,
            external_debug_info = make_artifact_tset(
                ctx.actions,
                label = ctx.label,
                artifacts = compile_output.external_debug_info,
                children = impl_params.additional.static_external_debug_info,
            ),
            pic = pic,
            optimized = optimized,
            debuggable = debuggable,
            stripped = stripped,
            extra_linkables = extra_static_linkables,
            bitcode_objects = compile_output.bitcode_objects,
            has_hip_device_debug = compile_output.has_hip_device_debug,
            hip_arch_debug_files = compile_output.hip_arch_debug_files,
        )

    def build_shared_library(
        compile_output: _CxxLibraryCompileOutput, flavor: LinkableFlavor
    ) -> (CxxLibraryOutput, _CxxSharedLibraryResult, NamedLinkedObject):
        external_debug_artifacts = compile_output.external_debug_info
        if compile_output.objects_have_external_debug_info:
            external_debug_artifacts.extend(compile_output.objects)
        if impl_params.extra_link_input_has_external_debug_info:
            external_debug_artifacts.extend(impl_params.extra_link_input)
        external_debug_info = make_artifact_tset(
            actions = ctx.actions,
            label = ctx.label,
            artifacts = external_debug_artifacts,
            children = impl_params.additional.shared_external_debug_info,
            tags = impl_params.additional.external_debug_info_tags,
        )

        result = _shared_library(
            ctx = ctx,
            impl_params = impl_params,
            objects = compile_output.objects,
            external_debug_info = external_debug_info,
            dep_infos = dep_infos,
            gnu_use_link_groups = gnu_use_link_groups,
            link_ordering = map_val(LinkOrdering, ctx.attrs.link_ordering),
            link_execution_preference = link_execution_preference,
            flavor = flavor,
            has_hip_device_debug = compile_output.has_hip_device_debug,
        )
        shlib = result.link_result.linked_object

        hip_debug = compile_output.hip_arch_debug_files
        if hip_debug:
            shlib = LinkedObject(
                output = shlib.output,
                bitcode_bundle = shlib.bitcode_bundle,
                unstripped_output = shlib.unstripped_output,
                prebolt_output = shlib.prebolt_output,
                link_args = shlib.link_args,
                dwp = shlib.dwp,
                hip_arch_debug_files = hip_debug,
                external_debug_info = shlib.external_debug_info,
                linker_argsfile = shlib.linker_argsfile,
                linker_command = shlib.linker_command,
                index_argsfile = shlib.index_argsfile,
                import_library = shlib.import_library,
                pdb = shlib.pdb,
                split_debug_output = shlib.split_debug_output,
            )

        extra_outputs = result.link_result.extra_outputs

        link_cmd_debug_output_file = None
        link_cmd_debug_output = make_link_command_debug_output(shlib)
        if link_cmd_debug_output != None and flavor == LinkableFlavor("default"):
            link_cmd_debug_output_file = make_link_command_debug_output_json_info(ctx.actions, [link_cmd_debug_output])
            providers.append(LinkCommandDebugOutputInfo(debug_outputs = [link_cmd_debug_output]))

        unstripped = shlib.unstripped_output
        output = CxxLibraryOutput(
            output_style = LibOutputStyle("shared_lib"),
            default = shlib.output,
            unstripped = unstripped,
            object_files = compile_output.objects,
            external_debug_info = shlib.external_debug_info,
            dwp = shlib.dwp,
            hip_arch_debug_files = hip_debug,
            linker_map = result.link_result.linker_map_data,
            sub_targets = extra_outputs
            | {
                "linker.argsfile": [
                    DefaultInfo(
                        default_output = shlib.linker_argsfile,
                    )
                ],
                "linker.command": [
                    DefaultInfo(
                        default_outputs = filter(None, [link_cmd_debug_output_file]),
                    )
                ],
                "unstripped": [
                    DefaultInfo(
                        default_output = unstripped,
                    )
                ],
            },
            pdb = shlib.pdb,
            implib = shlib.import_library,
        )

        solib = NamedLinkedObject(
            soname = result.soname,
            linked_object = shlib,
            extra_outputs = extra_outputs,
        )

        return (output, result, solib)

    # We don't know which outputs consumers may want, so we define all the possibilities given our preferred linkage.
    for output_style in get_output_styles_for_linkage(preferred_linkage):
        outputs_for_style = {}
        optimized_info = None
        debuggable_info = None
        stripped = None
        info = None

        # Generate the necessary libraries and
        # add them to the exported link info.
        if output_style != LibOutputStyle("shared_lib"):
            pic = _use_pic(output_style)
            lib_compile_output = compiled_srcs.pic
            if not pic:
                lib_compile_output = compiled_srcs.non_pic
                if not lib_compile_output:
                    fail("output_style {} requires non_pic compiled srcs, but didn't have any in {}".format(output_style, compiled_srcs))

            gcno_files += lib_compile_output.gcno_files

            # FYI(patskovn): Note that we are using `pic` in optimized and debuggable transformations here.
            #                Ideal way to set it up would be to expose whole matrix of debuggable/optimized/pic instead.
            #                But it increases retained memory (not much really, but still) so in the mean time I decied
            #                to not do it. As a downside, if transformation spec is used to build a binary statically
            #                without -fPIE, we are still using pic objects for it. I haven't found any real world impact
            #                of that though. Another point is: transformation specs are not used for building for production
            #                and only affect local development so we can do this experiment first. I may revisit that decision.
            optimized_output, optimized_info = build_static_library(
                compile_output = compiled_srcs.pic_optimized,
                pic = pic,
                optimized = True,
                debuggable = False,
                pch = False,
                stripped = False,
            )
            if optimized_output:
                outputs_for_style[LinkableFlavor("optimized")] = optimized_output

            debuggable_output, debuggable_info = build_static_library(
                compile_output = compiled_srcs.pic_debuggable,
                pic = pic,
                optimized = False,
                debuggable = True,
                pch = False,
                stripped = False,
            )
            if debuggable_output:
                outputs_for_style[LinkableFlavor("debug")] = debuggable_output

            # Only generate an archive if we have objects to include
            if lib_compile_output.objects or (compile_pch and lib_compile_output.pch_object_output):
                default_output, info = build_static_library(
                    compile_output = lib_compile_output,
                    pic = pic,
                    optimized = False,
                    debuggable = False,
                    pch = bool(compile_pch),
                    stripped = False,
                )
                stripped_output, stripped = _static_library(
                    ctx,
                    impl_params,
                    lib_compile_output.stripped_objects,
                    pic = pic,
                    stripped = True,
                    optimized = False,
                    debuggable = False,
                    extra_linkables = extra_static_linkables,
                    bitcode_objects = lib_compile_output.bitcode_objects,
                    has_hip_device_debug = lib_compile_output.has_hip_device_debug,
                    hip_arch_debug_files = lib_compile_output.hip_arch_debug_files,
                )
                if default_output:
                    outputs_for_style[LinkableFlavor("default")] = default_output
                if stripped_output:
                    outputs_for_style[LinkableFlavor("stripped")] = stripped_output
            else:
                # Header only libraries can have `extra_static_linkables`
                info = LinkInfo(
                    name = ctx.label.name,
                    linkables = extra_static_linkables,
                    metadata = cxx_attr_dep_metadata(ctx),
                )
        else:  # shared
            # Bare-metal toolchains don't support shared libraries (ld lacks -shared).
            # Provide the pic_archive as fallback to satisfy the linkable graph.
            if not get_cxx_toolchain_info(ctx).linker_info.supports_shared_libraries:
                if preferred_linkage == Linkage("shared"):
                    fail("{}: cannot build shared library for a toolchain that does not support shared libraries".format(ctx.label))
                link_infos[output_style] = link_infos[LibOutputStyle("pic_archive")]
                continue

            # If requested (by build_empty_so), we still generate a shared library even if there's no source objects.
            # This could be useful because it can still point to dependencies.
            # i.e. a rust_python_extension is an empty .so depending on a rust shared object
            compile_output = compiled_srcs.pic
            if compile_output.objects or impl_params.build_empty_so:
                gcno_files += compile_output.gcno_files

                default_output, result, solib = build_shared_library(
                    compile_output = compiled_srcs.pic,
                    flavor = LinkableFlavor("default"),
                )
                info = result.info
                outputs_for_style[LinkableFlavor("default")] = default_output
                solibs[LinkableFlavor("default")] = solib

                providers.append(result.link_result.link_execution_preference_info)

                link_sanitizer_runtime_files = result.link_result.sanitizer_runtime_files
                if link_sanitizer_runtime_files:
                    if sanitizer_runtime_files:
                        fail("Cannot specify sanitizer runtime files multiple times")
                    sanitizer_runtime_files = link_sanitizer_runtime_files

            if compiled_srcs.pic_optimized and (compiled_srcs.pic_optimized.objects or impl_params.build_empty_so):
                optimized_output, optimized_result, optimized_solib = build_shared_library(
                    compile_output = compiled_srcs.pic_optimized,
                    flavor = LinkableFlavor("optimized"),
                )
                optimized_info = optimized_result.info
                outputs_for_style[LinkableFlavor("optimized")] = optimized_output
                solibs[LinkableFlavor("optimized")] = optimized_solib

            if compiled_srcs.pic_debuggable and (compiled_srcs.pic_debuggable.objects or impl_params.build_empty_so):
                debug_output, debug_result, debug_solib = build_shared_library(
                    compile_output = compiled_srcs.pic_debuggable,
                    flavor = LinkableFlavor("debug"),
                )
                debuggable_info = debug_result.info
                outputs_for_style[LinkableFlavor("debug")] = debug_output
                solibs[LinkableFlavor("debug")] = debug_solib

        if outputs_for_style:
            outputs[output_style] = outputs_for_style

        # you cannot link against header only libraries so create an empty link info
        info = info if info != None else LinkInfo()
        link_infos[output_style] = LinkInfos(
            label = ctx.label,
            default = ldflags(info),
            optimized = ldflags(optimized_info) if optimized_info != None else None,
            debuggable = ldflags(debuggable_info) if debuggable_info != None else None,
            stripped = ldflags(stripped) if stripped != None else None,
        )

    if get_cxx_toolchain_info(ctx).gcno_files:
        deps_gcno_files = [x[GcnoFilesInfo].gcno_files for x in ctx.attrs.deps + ctx.attrs.exported_deps if GcnoFilesInfo in x]
        providers.append(
            GcnoFilesInfo(
                gcno_files = dedupe(flatten(deps_gcno_files) + gcno_files),
            )
        )

    return _CxxAllLibraryOutputs(
        outputs = outputs,
        link_infos = link_infos,
        providers = providers,
        solibs = solibs,
        sanitizer_runtime_files = sanitizer_runtime_files,
    )

def _strip_objects(
    actions: AnalysisActions, cxx_toolchain_info: CxxToolchainInfo, outs: list[CxxCompileOutput], supports_stripping: bool, has_content_based_path: bool
) -> list[Artifact]:
    """
    Return new objects with debug info stripped.
    """

    # Stripping is not supported on Windows
    linker_type = cxx_toolchain_info.linker_info.type
    if linker_type == LinkerType("windows"):
        return [out.object for out in outs]

    # Disable stripping if no `strip` binary was provided by the toolchain.
    if cxx_toolchain_info.binary_utilities_info == None or cxx_toolchain_info.binary_utilities_info.strip == None:
        return [out.object for out in outs]

    # Disable stripping if library opted out from it
    if not supports_stripping:
        return [out.object for out in outs]

    stripped_objects = []

    for out in outs:
        obj = out.object
        if out.hip_device_debug:
            # hip_debug_extract already rewrote the embedded GPU code objects;
            # a second strip invalidates ELF extended section indexes.
            stripped_objects.append(obj)
            continue
        base, ext = paths.split_extension(obj.short_path)
        expect(ext == ".o")
        stripped_objects.append(strip_debug_info(actions, base + ".stripped.o", obj, cxx_toolchain_info, has_content_based_path = has_content_based_path))

    return stripped_objects

_SharedLibraryLinks = record(
    link_args = field(LinkArgs),
    # The `[link-group-map-database]` output of a link group-aware link.
    link_group_map_database = field(DefaultInfo | None),
    # The `[link-groups-info]` output of a link group-aware link: every
    # linkable of the shared library's link in link order.
    link_groups_info = field(DefaultInfo | None),
    link_execution_preference = field(LinkExecutionPreference),
)

def _get_shared_library_links(
    ctx: AnalysisContext,
    reduce_linkable_graph_func: typing.Callable[[], ReducedLinkableGraph],
    link_group: [str, None],
    link_group_mappings: [dict[Label, str], None],
    link_group_preferred_linkage: dict[Label, Linkage],
    link_group_libs: dict[str, LinkGroupLib],
    exported_deps: list[Dependency],
    non_exported_deps: list[Dependency],
    force_link_group_linking,
    frameworks_linkable: [FrameworksLinkable, None],
    swiftmodule_linkable: [SwiftmoduleLinkable, None],
    force_static_follows_dependents: bool = True,
    link_groups_info_extras: bool = False,
) -> _SharedLibraryLinks:
    """
    Returns the link args of the library's shared output, with the link group
    map database of a link group-aware link and the link execution preference.

    TODO(T110378116): Omnibus linking always creates shared libraries by linking
    against shared dependencies. This is not true for link groups and possibly
    other forms of shared libraries. Ideally we consolidate this logic and
    propagate up only the expected links. Until we determine the comprehensive
    logic here, simply diverge behavior on whether link groups are defined.
    """

    pic_behavior = get_cxx_toolchain_info(ctx).pic_behavior

    # If we're not filtering for link groups, link against the shared dependencies
    if not link_group_mappings and not force_link_group_linking:
        deps = dedupe(flatten([non_exported_deps, exported_deps]))
        deps_merged_link_infos = cxx_inherited_link_info(deps)

        link_strategy = _link_strategy_for_some_shared_links(ctx.attrs)

        # We cannot support deriving link execution preference off the included links, as we've already
        # lost the information on what is in the link.
        # TODO(T152860998): Derive link_execution_preference based upon the included links
        # Not all rules calling `cxx_library_parameterized` have `link_execution_preference`. Notably `cxx_python_extension`.
        link_execution_preference = (
            get_link_execution_preference(ctx, []) if hasattr(ctx.attrs, "link_execution_preference") else LinkExecutionPreference("any")
        )

        return _SharedLibraryLinks(
            link_args = apple_build_link_args_with_deduped_flags(
                ctx,
                deps_merged_link_infos,
                frameworks_linkable,
                # fPIC behaves differently on various combinations of toolchains + platforms.
                # To get the link_strategy, we have to check the link_strategy against the toolchain's pic_behavior.
                #
                # For more info, check the PicBehavior docs.
                process_link_strategy_for_pic_behavior(link_strategy, pic_behavior),
                None,
                swiftmodule_linkable,
            ),
            link_group_map_database = None,
            link_groups_info = None,
            link_execution_preference = link_execution_preference,
        )

    # Else get filtered link group links
    prefer_stripped = cxx_is_gnu(ctx) and ctx.attrs.prefer_stripped_objects

    # TODO(cjhopman): Why is this different than where we compute just above for the non link-group case?
    link_strategy = to_link_strategy(cxx_attr_link_style(ctx))
    if link_strategy == LinkStrategy("static"):
        link_strategy = LinkStrategy("static_pic")
    link_strategy = process_link_strategy_for_pic_behavior(link_strategy, pic_behavior)
    reduced_linkable_graph = reduce_linkable_graph_func()

    roots = set(linkable_deps(non_exported_deps + exported_deps))
    is_executable_link = False
    lib_linkables = collect_linkables(
        reduced_linkable_graph,
        is_executable_link,
        link_strategy,
        link_group_preferred_linkage,
        pic_behavior,
        roots,
    )

    build_context = BuildLinkGroupsContext(
        public_nodes = set(),
        linkable_graph = reduced_linkable_graph,
        link_groups = {},
        link_group_mappings = link_group_mappings,
        link_group_preferred_linkage = link_group_preferred_linkage,
        link_strategy = link_strategy,
        pic_behavior = pic_behavior,
        link_group_libs = {name: (lib.label, lib.shared_link_infos) for name, lib in link_group_libs.items()},
        prefer_stripped = prefer_stripped,
        prefer_optimized = False,
    )

    filtered_labels_to_links = get_filtered_labels_to_links_map(
        link_group = link_group,
        linkables = lib_linkables,
        is_executable_link = False,
        build_context = build_context,
        force_static_follows_dependents = force_static_follows_dependents,
    )
    filtered_links = get_filtered_links(filtered_labels_to_links.map)
    filtered_targets = get_filtered_targets(filtered_labels_to_links.map)

    link_execution_preference = get_link_execution_preference(ctx, filtered_labels_to_links.map.keys())

    # Unfortunately, link_groups does not use MergedLinkInfo to represent the args
    # for the resolved nodes in the graph.
    additional_links = apple_get_link_info_by_deduping_link_infos(ctx, filtered_links, frameworks_linkable, swiftmodule_linkable)
    if additional_links:
        filtered_links.append(additional_links)

    return _SharedLibraryLinks(
        link_args = LinkArgs(infos = filtered_links),
        link_group_map_database = get_link_group_map_json(ctx, filtered_targets),
        link_groups_info = get_link_groups_info_json(ctx, filtered_labels_to_links.map, link_groups_info_extras),
        link_execution_preference = link_execution_preference,
    )

def _use_pic(output_style: LibOutputStyle) -> bool:
    """
    Whether this output style requires PIC objects.
    """
    return output_style != LibOutputStyle("archive")

# Create the objects/archive to use for static linking this rule.
# Returns a tuple of objects/archive to use as the default output for the link
# style(s) it's used in and the `LinkInfo` to export to dependents.
def _static_library(
    ctx: AnalysisContext,
    impl_params: CxxRuleConstructorParams,
    objects: list[Artifact],
    pic: bool,
    optimized: bool,
    debuggable: bool,
    stripped: bool,
    extra_linkables: list[[FrameworksLinkable, SwiftmoduleLinkable]],
    objects_have_external_debug_info: bool = False,
    external_debug_info: ArtifactTSet = ArtifactTSet(),
    bitcode_objects: [list[Artifact], None] = None,
    has_hip_device_debug: bool = False,
    hip_arch_debug_files: dict[str, list[Artifact]] = {},
) -> (CxxLibraryOutput, LinkInfo):
    if optimized and debuggable:
        fail("Pls pick optimized OR debuggable")
    if len(objects) == 0:
        fail("empty objects")

    # No reason to create a static library with just a single object file. We
    # still want to create a static lib to expose as the default output because
    # it's the contract/expectation of external clients of the cmd line
    # interface. Any tools consuming `buck build` outputs should get a
    # consistent output type when building a library, not static lib or object
    # file depending on number of source files.
    linker_info = get_cxx_toolchain_info(ctx).linker_info
    linker_type = linker_info.type

    base_name = _base_static_library_name(ctx, optimized, debuggable, stripped)
    name = _archive_name(base_name, pic = pic, extension = linker_info.static_library_extension)

    # If we have extra hidden deps of this target add them to the archive action
    # so they are forced to build for static library output.
    archive = make_archive(ctx, name, objects, impl_params.extra_hidden)

    bitcode_bundle = _bitcode_bundle(ctx, bitcode_objects, optimized, debuggable, pic, stripped)
    if False:
        # TODO(nga): bitcode_bundle.artifact
        def unknown():
            pass

        bitcode_bundle = unknown()
    if bitcode_bundle != None and bitcode_bundle.artifact != None:
        bitcode_artifact = bitcode_bundle.artifact
    else:
        bitcode_artifact = None

    if use_archives(ctx):
        linkable = ArchiveLinkable(
            archive = archive,
            bitcode_bundle = bitcode_artifact,
            linker_type = linker_type,
            link_whole = _attr_link_whole(ctx),
        )
    else:
        linkable = ObjectsLinkable(
            objects = objects,
            bitcode_bundle = bitcode_artifact,
            linker_type = linker_type,
            link_whole = _attr_link_whole(ctx),
        )

    post_flags = []

    if pic:
        post_flags.extend(linker_info.static_pic_dep_runtime_ld_flags or [])
    else:
        post_flags.extend(linker_info.static_dep_runtime_ld_flags or [])

    # On darwin, the linked output references the archive that contains the
    # object files instead of the originating objects.
    object_external_debug_info = []
    if linker_type == LinkerType("darwin"):
        if archive.archive_contents_type == ArchiveContentsType("virtual"):
            # If using thin archives or passing archives --start-lib --end-lib style (virtual), we only need to provide the underlying objects
            object_external_debug_info.extend(archive.external_objects)
        elif archive.archive_contents_type == ArchiveContentsType("thin"):
            # If passing the archive to the linker conventionally, we need to provide both the thin archive
            # and the underlying object files if using thin archives
            object_external_debug_info.append(archive.artifact)
            object_external_debug_info.extend(archive.external_objects)
        else:
            # And only the archive if using regular archives
            object_external_debug_info.append(archive.artifact)
    elif archive.archive_contents_type == ArchiveContentsType("thin") and objects_have_external_debug_info:
        object_external_debug_info.extend(objects)

    all_external_debug_info = make_artifact_tset(
        actions = ctx.actions,
        label = ctx.label,
        artifacts = object_external_debug_info,
        children = [external_debug_info],
        tags = impl_params.additional.external_debug_info_tags,
    )

    return (
        CxxLibraryOutput(
            output_style = LibOutputStyle("pic_archive") if pic else LibOutputStyle("archive"),
            default = archive.artifact,
            unstripped = archive.artifact,
            object_files = objects,
            bitcode_bundle = bitcode_bundle,
            other = (archive.external_objects if archive.archive_contents_type == ArchiveContentsType("thin") else []),
            sub_targets = {},
        ),
        LinkInfo(
            name = name,
            # We're propagating object code for linking up the dep tree,
            # so we need to also propagate any necessary link flags required for
            # the object code.
            pre_flags = impl_params.extra_exported_link_flags,
            post_flags = post_flags,
            # Extra linkables are propagated here so they are available to link_groups
            # when they are deducing linker args.
            linkables = [linkable] + extra_linkables,
            external_debug_info = all_external_debug_info,
            has_hip_device_debug = has_hip_device_debug,
            hip_arch_debug_files = hip_arch_debug_files,
            metadata = cxx_attr_dep_metadata(ctx),
        ),
    )

# A bitcode bundle is very much like a static library and is generated from object file
# inputs, except the output is a combined bitcode file, which is not machine code.
def _bitcode_bundle(
    ctx: AnalysisContext, objects: [list[Artifact], None], optimized: bool, debuggable: bool, pic: bool, stripped: bool, name_extra = ""
) -> [BitcodeBundle, None]:
    if objects == None or len(objects) == 0:
        return None

    base_name = _base_static_library_name(ctx, optimized, debuggable, stripped = False)
    name = name_extra + _bitcode_bundle_name(base_name, pic, stripped)
    return make_bitcode_bundle(ctx, name, objects)

def _shared_library(
    ctx: AnalysisContext,
    impl_params: CxxRuleConstructorParams,
    objects: list[Artifact],
    external_debug_info: ArtifactTSet,
    dep_infos: LinkArgs,
    gnu_use_link_groups: bool,
    link_execution_preference: LinkExecutionPreference,
    link_ordering: [LinkOrdering, None],
    flavor: LinkableFlavor,
    has_hip_device_debug: bool = False,
) -> _CxxSharedLibraryResult:
    """
    Generate a shared library and the associated native link info used by
    dependents to link against it.
    """

    def flavored_output(output: str, separator: str = "/") -> str:
        if flavor != LinkableFlavor("default"):
            return flavor.value + separator + output
        return output

    soname = _soname(ctx, impl_params)
    cxx_toolchain = get_cxx_toolchain_info(ctx)
    linker_info = cxx_toolchain.linker_info

    local_bitcode_bundle = _bitcode_bundle(
        ctx,
        objects,
        optimized = flavor == LinkableFlavor("optimized"),
        debuggable = flavor == LinkableFlavor("debug"),
        pic = False,
        stripped = flavor == LinkableFlavor("stripped"),
        name_extra = "objects-",
    )

    # NOTE(agallagher): We add exported link flags here because it's what v1
    # does, but the intent of exported link flags are to wrap the link output
    # that we propagate up the tree, rather than being used locally when
    # generating a link product.
    linker_flags = cxx_attr_linker_flags_all(ctx, impl_params.cxx_flags)
    link_info = LinkInfo(
        dist_thin_lto_codegen_flags = getattr(ctx.attrs, "dist_thin_lto_codegen_flags", []),
        pre_flags = (linker_flags.flags + linker_flags.exported_flags + getattr(ctx.attrs, "local_linker_flags", [])),
        linkables = [
            ObjectsLinkable(
                objects = objects,
                bitcode_bundle = local_bitcode_bundle.artifact if local_bitcode_bundle else None,
                linker_type = linker_info.type,
                link_whole = True,
            )
        ],
        post_flags = (
            impl_params.extra_exported_link_flags + impl_params.extra_link_flags + linker_flags.post_flags + (linker_info.shared_dep_runtime_ld_flags or [])
            # TODO(cjhopman): Why doesn't this add exported_linker_flags.post_flags?
        ),
        external_debug_info = external_debug_info,
        metadata = cxx_attr_dep_metadata(ctx),
    )

    # If we have extra hidden deps here, add them as hidden inputs
    # to the link action so that they are forced to build before linking.
    links = [LinkArgs(infos = [link_info]), dep_infos]
    if impl_params.extra_hidden:
        links.append(
            LinkArgs(flags = cmd_args(hidden = impl_params.extra_hidden)),
        )

    # shared library interface generation can be disabled at the target level
    effective_shlib_interfaces_mode = ShlibInterfacesMode("disabled")
    if cxx_use_shlib_intfs(ctx):
        effective_shlib_interfaces_mode = get_cxx_toolchain_info(ctx).linker_info.shlib_interfaces
        if effective_shlib_interfaces_mode == ShlibInterfacesMode("defined_only") and gnu_use_link_groups:
            # TODO(agallagher): There's a bug in shlib intfs interacting with link
            # groups, where we don't include the symbols we're meant to export from
            # deps that get statically linked in.
            effective_shlib_interfaces_mode = ShlibInterfacesMode("disabled")

    needs_extract = hip_debug_extract_available(cxx_toolchain) and (has_hip_device_debug or link_args_have_hip_device_debug(links))
    link_output = flavored_output(soname) + PRE_EXTRACT_SUFFIX if needs_extract else flavored_output(soname)

    link_result = cxx_link_shared_library(
        ctx = ctx,
        output = link_output,
        opts = link_options(
            enable_distributed_thinlto = getattr(ctx.attrs, "enable_distributed_thinlto", False),
            links = links,
            identifier = flavored_output(soname, separator = "_"),
            link_ordering = link_ordering,
            strip = impl_params.strip_executable,
            strip_args_factory = impl_params.strip_args_factory,
            link_execution_preference = link_execution_preference,
            error_handler = impl_params.error_handler,
            extra_linker_outputs_factory = impl_params.extra_linker_outputs_factory,
            extra_linker_outputs_flags_factory = impl_params.extra_linker_outputs_flags_factory,
            extra_distributed_thin_lto_opt_outputs_merger = impl_params.extra_distributed_thin_lto_opt_outputs_merger,
            produce_shared_library_interface = effective_shlib_interfaces_mode in _INTERFACE_FROM_LIBRARY_LINK_MODES,
            has_hip_device_debug = has_hip_device_debug,
        ),
        name = soname if impl_params.use_soname else None,
        shared_library_flags = impl_params.shared_library_flags,
    )
    exported_shlib = link_result.linked_object.output

    if effective_shlib_interfaces_mode in _INTERFACE_FROM_LIBRARY_LINK_MODES:
        # Linking rdeps against the interface prevents relinking them when
        # changes do not affect the library's symbols.
        exported_shlib = link_result.shared_library_interface
    elif effective_shlib_interfaces_mode == ShlibInterfacesMode("defined_only"):
        link_info = LinkInfo(
            pre_flags = link_info.pre_flags,
            linkables = link_info.linkables,
            post_flags = (
                (link_info.post_flags or [])
                + get_ignore_undefined_symbols_flags(linker_info.type)
                + (linker_info.independent_shlib_interface_linker_flags or [])
            ),
            external_debug_info = link_info.external_debug_info,
            metadata = cxx_attr_dep_metadata(ctx),
        )
        intf_link_result = cxx_link_shared_library(
            ctx = ctx,
            output = flavored_output(
                get_shared_library_name(
                    linker_info,
                    ctx.label.name + "-for-interface",
                    apply_default_prefix = True,
                )
            ),
            opts = link_options(
                category_suffix = "interface",
                link_ordering = link_ordering,
                links = [LinkArgs(infos = [link_info])],
                identifier = flavored_output(soname + "-interface", separator = "_"),
                link_execution_preference = link_execution_preference,
                error_handler = impl_params.error_handler,
                produce_shared_library_interface = True,
            ),
            name = soname,
        )
        exported_shlib = intf_link_result.shared_library_interface

    # Link against import library on Windows.
    if link_result.linked_object.import_library:
        exported_shlib = link_result.linked_object.import_library

    return _CxxSharedLibraryResult(
        link_result = link_result,
        soname = soname,
        info = LinkInfo(
            name = soname,
            linkables = [
                SharedLibLinkable(
                    lib = exported_shlib,
                )
            ],
            has_hip_device_debug = has_hip_device_debug,
            metadata = cxx_attr_dep_metadata(ctx),
        ),
    )

def _attr_reexport_all_header_dependencies(ctx: AnalysisContext) -> bool:
    return value_or(ctx.attrs.reexport_all_header_dependencies, False)

def _soname(ctx: AnalysisContext, impl_params) -> str:
    """
    Get the shared library name to set for the given C++ library.
    """
    linker_info = get_cxx_toolchain_info(ctx).linker_info
    explicit_soname = value_or(ctx.attrs.soname, impl_params.soname)
    if explicit_soname != None:
        return get_shared_library_name_for_param(linker_info, explicit_soname)
    return get_default_shared_library_name(linker_info, ctx.label)

def _base_static_library_name(ctx: AnalysisContext, optimized: bool, debuggable: bool, stripped: bool) -> str:
    return "{}{}{}{}".format(ctx.label.name, ".optimized" if optimized else "", ".debuggable" if debuggable else "", ".stripped" if stripped else "")

def _archive_name(name: str, pic: bool, extension: str) -> str:
    return "lib{}{}.{}".format(name, ".pic" if pic else "", extension)

def _bitcode_bundle_name(name: str, pic: bool, stripped: bool = False) -> str:
    return "{}{}{}.bc".format(name, ".pic" if pic else "", ".stripped" if stripped else "")

def _attr_link_whole(ctx: AnalysisContext) -> bool:
    return value_or(ctx.attrs.link_whole, False)

def use_archives(ctx: AnalysisContext) -> bool:
    """
    Whether this rule should use archives to package objects when producing
    link input for dependents.
    """
    linker_info = get_cxx_toolchain_info(ctx).linker_info
    requires_archives = linker_info.requires_archives
    requires_objects = linker_info.requires_objects

    if requires_archives and requires_objects:
        fail("In cxx linker_info, only one of `requires_archives` and `requires_objects` can be enabled")

    # If the toolchain requires them, then always use them.
    if requires_archives:
        return True

    if requires_objects:
        return False

    # Otherwise, fallback to the rule-specific setting.
    return value_or(getattr(ctx.attrs, "use_archive", True), True)
