# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactInfoTag",
    "ArtifactTSet",
    "make_artifact_tset",
)
load("@prelude//apple:apple_stripping.bzl", "apple_strip_args")
load("@prelude//apple:apple_utility.bzl", "get_base_swiftinterface_compilation_flags")
load("@prelude//apple/swift:apple_sdk_modules_utility.bzl", "is_sdk_modules_provided")
load(
    "@prelude//apple/swift:swift_compilation.bzl",
    "create_swift_dependency_info",
    "get_external_debug_info_tsets",
    "get_external_swift_ast_dump_tsets",
    "get_external_swiftmodule_change_analysis_tsets",
    "get_swift_anonymous_targets_for_prebuilt_framework",
)
load(
    "@prelude//apple/swift:swift_incremental_support.bzl",
    "get_uses_content_based_paths",
)
load("@prelude//apple/swift:swift_pcm_compilation.bzl", "compile_framework_pcm")
load(
    "@prelude//apple/swift:swift_pcm_compilation_types.bzl",
    "SwiftPCMUncompiledInfo",
)
load("@prelude//apple/swift:swift_swiftinterface_compilation.bzl", "compile_swiftinterface_common")
load("@prelude//apple/swift:swift_toolchain_types.bzl", "SwiftCompiledModuleTset", "SwiftToolchainInfo")
load("@prelude//apple/swift:swift_types.bzl", "FrameworkImplicitSearchPathInfo", "SwiftDependencyInfo", "get_implicit_framework_search_path_providers")
load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load(
    "@prelude//cxx:cxx_library_utility.bzl",
    "cxx_attr_preferred_linkage",
    "cxx_platform_supported",
)
load(
    "@prelude//cxx:preprocessor.bzl",
    "CPreprocessor",
    "CPreprocessorArgs",
    "cxx_inherited_preprocessor_infos",
    "cxx_merge_cpreprocessors",
)
load("@prelude//cxx:target_sdk_version.bzl", "get_target_triple", "get_unversioned_target_triple")
load(
    "@prelude//linking:link_groups.bzl",
    "merge_link_group_lib_info",
)
load(
    "@prelude//linking:link_info.bzl",
    "Archive",
    "ArchiveLinkable",
    "LibOutputStyle",
    "LinkInfo",
    "LinkInfos",
    "SharedLibLinkable",
    "create_merged_link_info",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "create_linkable_graph",
    "create_linkable_graph_node",
    "create_linkable_node",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
    "merge_shared_libraries",
)
load("@prelude//linking:strip.bzl", "strip_object")
load("@prelude//linking:types.bzl", "Linkage")
load("@prelude//utils:utils.bzl", "filter_and_map_idx")
load(":apple_bundle_types.bzl", "AppleBundleInfo", "AppleBundleTypeDefault")
load(":apple_dsym.bzl", "DSYM_SUBTARGET")
load(":apple_frameworks.bzl", "to_framework_name")
load(":apple_toolchain_types.bzl", "AppleToolsInfo")
load(":apple_utility.bzl", "get_apple_stripped_attr_value_with_default_fallback")
load(":debug.bzl", "AppleDebuggableInfo")

def _get_compiled_swift_deps_tset(ctx: AnalysisContext, deps_providers: list) -> SwiftCompiledModuleTset:
    deps = [d[SwiftDependencyInfo].exported_swiftmodules for d in deps_providers if SwiftDependencyInfo in d]
    return ctx.actions.tset(SwiftCompiledModuleTset, children = deps)

def prebuilt_apple_framework_impl(ctx: AnalysisContext) -> [list[Provider], Promise]:
    def get_prebuilt_apple_framework_providers(deps_providers) -> list[Provider]:
        providers = []

        framework_directory_artifact = ctx.attrs.framework
        framework_name = to_framework_name(framework_directory_artifact.basename)
        framework_binary_name = ctx.attrs.binary if ctx.attrs.binary else framework_name
        framework_library_artifact = framework_directory_artifact.project(framework_binary_name)

        # Check this rule's `supported_platforms_regex` with the current platform.
        if cxx_platform_supported(ctx):
            uses_content_based_path = get_uses_content_based_paths(ctx)

            # Sandbox the framework, to avoid leaking other frameworks via search paths.
            framework_dir = ctx.actions.symlinked_dir(
                "Frameworks",
                {framework_name + ".framework": framework_directory_artifact},
                has_content_based_path = uses_content_based_path,
            )

            # Add framework & pp info from deps.
            inherited_pp_info = cxx_inherited_preprocessor_infos(ctx.attrs.deps)
            providers.append(
                cxx_merge_cpreprocessors(
                    ctx.actions,
                    [CPreprocessor(args = CPreprocessorArgs(args = ["-F", framework_dir]))],
                    inherited_pp_info,
                )
            )

            if cxx_attr_preferred_linkage(ctx) == Linkage("static"):
                linkable = ArchiveLinkable(
                    archive = Archive(artifact = framework_library_artifact),
                    linker_type = get_cxx_toolchain_info(ctx).linker_info.type,
                )
            else:
                # If unspecified we default to "shared".
                linkable = SharedLibLinkable(
                    lib = framework_library_artifact,
                )

            link = LinkInfo(
                name = framework_name,
                linkables = [linkable],
                pre_flags = [ctx.attrs.exported_linker_flags],
            )
            link_info = LinkInfos(default = link)

            providers.append(
                create_merged_link_info(
                    ctx,
                    get_cxx_toolchain_info(ctx).pic_behavior,
                    {output_style: link_info for output_style in LibOutputStyle},
                )
            )

            # Create, augment and provide the linkable graph.
            linkable_graph = create_linkable_graph(
                ctx,
                node = create_linkable_graph_node(
                    ctx,
                    linkable_node = create_linkable_node(
                        ctx,
                        link_infos = {output_style: link_info for output_style in LibOutputStyle},
                        # TODO(cjhopman): this should be set to non-None
                        default_soname = None,
                    ),
                    excluded = {ctx.label: None},
                ),
            )
            providers.append(linkable_graph)

        providers.append(merge_link_group_lib_info(deps = ctx.attrs.deps))
        providers.append(merge_shared_libraries(ctx.actions, deps = filter_and_map_idx(SharedLibraryInfo, ctx.attrs.deps)))

        # The default output is the provided framework.
        sub_targets = {
            "distribution": _sanitize_framework_for_app_distribution(ctx, framework_directory_artifact) + providers,
        }

        if ctx.attrs.dsyms:
            sub_targets[DSYM_SUBTARGET] = [DefaultInfo(default_outputs = ctx.attrs.dsyms)]
            providers.append(AppleDebuggableInfo(dsyms = ctx.attrs.dsyms, debug_info_tset = ArtifactTSet()))

        providers.append(DefaultInfo(default_output = framework_directory_artifact, sub_targets = sub_targets))
        providers.append(
            AppleBundleInfo(
                bundle = framework_directory_artifact,
                bundle_type = AppleBundleTypeDefault,
                skip_copying_swift_stdlib = True,
                contains_watchapp = None,
                extra_codesign_paths = ctx.attrs.extra_codesign_paths,
            )
        )

        if ctx.attrs.modular:
            pcm_provider = _create_uncompiled_pcm_module_info(ctx, framework_directory_artifact, framework_name)
            providers.append(pcm_provider)

            # Since not all frameworks expose a swiftinterface, we use the `contains_swift` attribute to determine if one is available.
            if ctx.attrs.contains_swift:
                swift_dependency_info = _compile_swiftinterface(
                    ctx,
                    framework_name,
                    pcm_provider,
                    deps_providers,
                    framework_directory_artifact,
                )
                providers.append(swift_dependency_info)

        implicit_search_path_tset = get_implicit_framework_search_path_providers(
            ctx,
            cmd_args("-F", cmd_args(framework_directory_artifact, parent = 1)),
            ctx.attrs.deps,
        )

        providers.append(
            FrameworkImplicitSearchPathInfo(tset = implicit_search_path_tset),
        )

        return providers

    # We cannot determine whether Swift Explicit modules are enabled at this point.
    # Therefore, we always return providers for both Implicit and Explicit modules, if SDK modules are available.
    # This approach is safe and won't trigger compilation of swiftinterfaces or pcm modules,
    # as no upper-level targets will depend on the artifacts from these compilations.
    swift_toolchain = ctx.attrs._apple_toolchain[SwiftToolchainInfo]
    if is_sdk_modules_provided(swift_toolchain):
        return get_swift_anonymous_targets_for_prebuilt_framework(ctx, get_prebuilt_apple_framework_providers)
    else:
        return get_prebuilt_apple_framework_providers([])

def _create_uncompiled_pcm_module_info(ctx: AnalysisContext, framework_directory_artifact: Artifact, framework_name: str) -> SwiftPCMUncompiledInfo:
    modulemap_artifact = framework_directory_artifact.project("Modules/module.modulemap").with_associated_artifacts([framework_directory_artifact])
    clang_importer_args = cmd_args(framework_directory_artifact, parent = 1, format = "-F{}")
    return SwiftPCMUncompiledInfo(
        clang_importer_args = cmd_args(),
        exported_clang_importer_args = clang_importer_args,
        exported_deps = ctx.attrs.deps,
        is_transient = False,
        modulemap_artifact = modulemap_artifact,
        name = framework_name,
        uncompiled_sdk_modules = ctx.attrs.sdk_modules,
    )

def _compile_swiftinterface(
    ctx: AnalysisContext, framework_name: str, pcm_provider: SwiftPCMUncompiledInfo, deps_providers, framework_directory_artifact: Artifact
) -> SwiftDependencyInfo:
    # To compile the framework's swiftinterface, the PCM module must be precompiled first.
    compiled_underlying_pcm = compile_framework_pcm(
        ctx,
        framework_name,
        pcm_provider,
        deps_providers,
        ["-target", get_target_triple(ctx)],
    )

    partial_cmd = get_base_swiftinterface_compilation_flags(framework_name)

    swiftinterface_path = cmd_args(
        framework_directory_artifact,
        "/Modules/",
        framework_name,
        ".swiftmodule/" + get_unversioned_target_triple(ctx) + ".swiftinterface",
        delimiter = "",
    )

    swift_third_party_deps = _get_compiled_swift_deps_tset(ctx, ctx.attrs.deps)

    swift_compiled_module, _ = compile_swiftinterface_common(
        ctx,
        ctx.attrs.deps,
        True,  # is_framework
        framework_name,
        partial_cmd,
        deps_providers,
        swiftinterface_path,
        "prebuilt_framework_swiftinterface_compilation",
        compiled_underlying_pcm,
        additional_compiled_swiftmodules = swift_third_party_deps,
    )

    debug_info_tset = make_artifact_tset(
        actions = ctx.actions,
        artifacts = [swift_compiled_module.output_artifact, compiled_underlying_pcm.output_artifact],
        children = get_external_debug_info_tsets(False, ctx.attrs.deps),
        label = ctx.label,
        tags = [ArtifactInfoTag("swift_debug_info")],
    )

    swiftmodule_change_analysis_tset = make_artifact_tset(
        actions = ctx.actions,
        # Just the swiftmodule, not the underlying PCM (unlike debug_info_tset
        # above): PCM-only changes aren't swiftmodule changes, and including it
        # would re-run this analysis action whenever the PCM changes for no
        # reason.
        artifacts = [swift_compiled_module.output_artifact],
        children = get_external_swiftmodule_change_analysis_tsets(False, ctx.attrs.deps),
        label = ctx.label,
    )

    swift_ast_dump_tset = make_artifact_tset(
        actions = ctx.actions,
        children = get_external_swift_ast_dump_tsets(False, ctx.attrs.deps),
        label = ctx.label,
    )

    swift_dependency_info = create_swift_dependency_info(
        ctx,
        ctx.attrs.deps,
        deps_providers,
        swift_compiled_module,
        debug_info_tset,
        swiftmodule_change_analysis_tset,
        swift_ast_dump_tset,
        False,
    )

    return swift_dependency_info

def _sanitize_framework_for_app_distribution(ctx: AnalysisContext, framework_directory_artifact: Artifact) -> list[Provider]:
    framework_name = to_framework_name(framework_directory_artifact.basename)
    bundle_for_app_distribution = ctx.actions.declare_output(framework_name + ".framework", dir = True, has_content_based_path = False)

    apple_tools = ctx.attrs._apple_tools[AppleToolsInfo]
    framework_sanitize_command = cmd_args([
        apple_tools.framework_sanitizer,
        "--input",
        framework_directory_artifact,
        "--output",
        bundle_for_app_distribution.as_output(),
    ])

    if get_apple_stripped_attr_value_with_default_fallback(ctx):
        strip_args = apple_strip_args(ctx)
        stripped = strip_object(ctx, get_cxx_toolchain_info(ctx), framework_directory_artifact.project(framework_name), strip_args, "framework_distribution")
        framework_sanitize_command.add("--replacement-binary", stripped)

    ctx.actions.run(framework_sanitize_command, category = "sanitize_prebuilt_apple_framework")
    providers = [DefaultInfo(default_output = bundle_for_app_distribution)]
    providers.append(
        AppleBundleInfo(
            bundle = bundle_for_app_distribution,
            bundle_type = AppleBundleTypeDefault,
            skip_copying_swift_stdlib = True,
            contains_watchapp = None,
            extra_codesign_paths = ctx.attrs.extra_codesign_paths,
        )
    )
    return providers
