# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//:validation_deps.bzl", "get_validation_deps_outputs")
load("@prelude//apple:apple_stripping.bzl", "apple_strip_args")
load("@prelude//apple:apple_utility.bzl", "get_module_name")
load(
    "@prelude//apple:modularization_dependency_graph.bzl",
    "create_modularization_dep_graph_subtargets_and_provider",
)
load("@prelude//apple:xcassets_asset_symbols.bzl", "xcassets_asset_symbol_usage_providers_and_subtargets")
# @oss-disable[end= ]: load(
    # @oss-disable[end= ]: "@prelude//apple/meta_only:linker_outputs.bzl",
    # @oss-disable[end= ]: "extra_distributed_thin_lto_opt_outputs_merger",
    # @oss-disable[end= ]: "get_extra_linker_output_flags",
    # @oss-disable[end= ]: "get_extra_linker_outputs",
# @oss-disable[end= ]: )
load(
    "@prelude//apple/swift:swift_compilation.bzl",
    "compile_swift",
    "get_swift_anonymous_targets",
    "get_swift_debug_infos",
    "get_swift_dependency_info",
    "get_swiftmodule_linkable",
)
load("@prelude//apple/swift:swift_helpers.bzl", "uses_explicit_modules")
load(
    "@prelude//apple/swift:swift_incremental_support.bzl",
    "get_uses_content_based_paths",
)
load("@prelude//apple/swift:swift_types.bzl", "SWIFT_EXTENSION")
load(
    "@prelude//cxx:argsfiles.bzl",
    "CompileArgsfiles",
)
load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load("@prelude//cxx:cxx_executable.bzl", "cxx_executable")
load("@prelude//cxx:cxx_flags.bzl", "cxx_attr_flags")
load("@prelude//cxx:cxx_library_utility.bzl", "cxx_attr_deps", "cxx_attr_exported_deps", "cxx_attr_link_style")
load(
    "@prelude//cxx:cxx_sources.bzl",
    "CxxSrcWithFlags",  # @unused Used as a type
    "get_srcs_with_flags",
)
load(
    "@prelude//cxx:cxx_transitive_diagnostics.bzl",
    "cxx_transitive_diagnostics_combine",
)
load(
    "@prelude//cxx:cxx_types.bzl",
    "CxxRuleAdditionalParams",
    "CxxRuleConstructorParams",
    "CxxRuleSubTargetParams",
    "xcode_data_enabled",
)
load("@prelude//cxx:cxx_utility.bzl", "cxx_attrs_get_allow_cache_upload")
load(
    "@prelude//cxx:headers.bzl",
    "HeaderMode",
    "cxx_attr_headers",
    "cxx_attr_headers_list",
    "cxx_get_regular_cxx_headers_layout",
    "prepare_headers",
)
load("@prelude//cxx:index_store.bzl", "create_index_store_subtargets_and_provider")
load(
    "@prelude//cxx:link_groups.bzl",
    "get_link_group_info",
)
load("@prelude//cxx:link_types.bzl", "ExtraLinkerOutputCategory")
load(
    "@prelude//linking:link_info.bzl",
    "CxxSanitizerRuntimeInfo",
    "ExtraLinkerOutputs",
    "LinkCommandDebugOutputInfo",
    "UnstrippedLinkOutputInfo",
    "to_link_strategy",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "LinkableGraph",
)
load("@prelude//target_stats:target_stats.bzl", "CycleMode", "target_stats_providers_and_subtargets")
load("@prelude//target_stats:target_stats_config.bzl", "TARGET_STATS_ENABLED")
load("@prelude//utils:arglike.bzl", "ArgLike")
load("@prelude//utils:utils.bzl", "filter_and_map_idx", "map_val")
load(":apple_bundle_types.bzl", "AppleBundleLinkerMapInfo", "AppleMinDeploymentVersionInfo")
load(":apple_bundle_utility.bzl", "get_bundle_infos_from_graph", "merge_bundle_linker_maps_info")
load(":apple_code_signing_types.bzl", "AppleEntitlementsInfo")
load(":apple_dsym.bzl", "DSYM_SUBTARGET", "get_apple_dsym")
load(":apple_entitlements.bzl", "entitlements_link_flags")
load(":apple_error_handler.bzl", "apple_build_error_handler", "cxx_error_deserializer", "cxx_error_handler")
load(":apple_frameworks.bzl", "get_framework_search_path_flags")
load(":apple_rpaths.bzl", "get_rpath_flags_for_apple_binary")
load(":apple_target_sdk_version.bzl", "get_min_deployment_version_for_node")
load(":apple_utility.bzl", "get_apple_cxx_headers_layout", "get_apple_stripped_attr_value_with_default_fallback", "target_stats_header_name")
load(":debug.bzl", "AppleDebuggableInfo")
load(":resource_groups.bzl", "create_resource_graph")
load(":xcode.bzl", "apple_populate_xcode_attributes")

def apple_binary_impl(ctx: AnalysisContext) -> [list[Provider], Promise]:
    def get_apple_binary_providers(deps_providers) -> list[Provider]:
        # FIXME: Ideally we'd like to remove the support of "bridging header",
        # cause it affects build time and in general considered a bad practise.
        # But we need it for now to achieve compatibility with BUCK1.
        objc_bridging_header_flags = _get_bridging_header_flags(ctx)

        cxx_srcs, swift_srcs = _filter_swift_srcs(ctx)
        contains_swift_sources = len(swift_srcs) > 0

        module_name = get_module_name(ctx)

        framework_search_path_flags = get_framework_search_path_flags(ctx)
        swift_compile_result = compile_swift(
            ctx,
            swift_srcs,
            False,  # parse_as_library
            deps_providers,
            module_name,
            [],
            None,
            framework_search_path_flags,
            objc_bridging_header_flags,
        )
        swift_compile = swift_compile_result.swift_compilation
        swift_object_files = swift_compile.object_files if swift_compile else []
        swift_preprocessor = [swift_compile.pre] if swift_compile else []
        extra_link_flags = get_rpath_flags_for_apple_binary(ctx) + entitlements_link_flags(ctx)

        # Extensions have custom entry points and API restrictions
        extension_compiler_flags = []
        if ctx.attrs.application_extension:
            extra_link_flags += [
                "-fapplication-extension",
                "-e",
                "_NSExtensionMain",
            ]
            extension_compiler_flags = ["-fapplication-extension"]

        swift_dependency_info = swift_compile.dependency_info if swift_compile else get_swift_dependency_info(ctx, None, deps_providers, False)
        swift_debug_info = get_swift_debug_infos(
            ctx,
            swift_dependency_info,
            swift_compile,
        )

        link_strategy = to_link_strategy(cxx_attr_link_style(ctx))
        link_group_info = get_link_group_info(
            ctx,
            filter_and_map_idx(LinkableGraph, cxx_attr_deps(ctx)),
            link_strategy,
        )
        binary_subtargets = {
            "swift-compilation-database": [
                DefaultInfo(
                    default_output = swift_compile.compilation_database.db if swift_compile else None,
                    other_outputs = [swift_compile.compilation_database.other_outputs] if swift_compile else [],
                ),
            ],
        }
        if xcode_data_enabled():
            # The objects of the binary's own Swift sources, which the link takes as
            # `extra_link_input` and `[objects]` (the C-family objects) does not list.
            binary_subtargets["swift-objects"] = [DefaultInfo(default_outputs = swift_object_files)]

        validation_deps_outputs = get_validation_deps_outputs(ctx)
        stripped = get_apple_stripped_attr_value_with_default_fallback(ctx)
        constructor_params = CxxRuleConstructorParams(
            rule_type = "apple_binary",
            generate_sub_targets = CxxRuleSubTargetParams(xcode_data = xcode_data_enabled()),
            headers_layout = get_apple_cxx_headers_layout(ctx),
            extra_link_flags = extra_link_flags,
            extra_hidden = validation_deps_outputs,
            srcs = cxx_srcs,
            additional = CxxRuleAdditionalParams(
                srcs = swift_srcs,
                argsfiles = swift_compile.argsfiles if swift_compile else CompileArgsfiles(),
                # We need to add any swift modules that we include in the link, as
                # these will end up as `N_AST` entries that `dsymutil` will need to
                # follow.
                static_external_debug_info = swift_debug_info.static,
                shared_external_debug_info = swift_debug_info.shared,
                subtargets = binary_subtargets,
            ),
            extra_link_input = swift_object_files,
            extra_link_input_has_external_debug_info = True,
            extra_preprocessors = swift_preprocessor,
            strip_executable = stripped,
            strip_args_factory = apple_strip_args,
            cxx_populate_xcode_attributes_func = lambda local_ctx, **kwargs: apple_populate_xcode_attributes(
                local_ctx, contains_swift_sources = contains_swift_sources, **kwargs
            ),
            link_group_info = link_group_info,
            prefer_stripped_objects = ctx.attrs.prefer_stripped_objects,
            # Some apple rules rely on `static` libs *not* following dependents.
            link_groups_force_static_follows_dependents = False,
            swiftmodule_linkable = get_swiftmodule_linkable(swift_compile),
            compiler_flags = ctx.attrs.compiler_flags + extension_compiler_flags,
            cxx_flags = cxx_attr_flags(ctx),
            lang_compiler_flags = ctx.attrs.lang_compiler_flags,
            preprocessor_flags = ctx.attrs.preprocessor_flags,
            lang_preprocessor_flags = ctx.attrs.lang_preprocessor_flags,
            error_handler = cxx_error_handler if cxx_error_deserializer(ctx) else apple_build_error_handler,
            index_stores = [swift_compile.index_store] if swift_compile else None,
            executable_name = ctx.attrs.executable_name,
            extra_linker_outputs_factory = _get_extra_linker_outputs,
            extra_linker_outputs_flags_factory = _get_extra_linker_outputs_flags,
            extra_distributed_thin_lto_opt_outputs_merger = _extra_distributed_thin_lto_opt_outputs_merger,
            allow_cache_upload = cxx_attrs_get_allow_cache_upload(ctx.attrs, get_cxx_toolchain_info(ctx).cxx_compiler_info.allow_cache_upload),
            precompiled_header = ctx.attrs.precompiled_header,
            prefix_header = ctx.attrs.prefix_header,
            use_content_based_paths = get_uses_content_based_paths(ctx),
        )
        cxx_output = cxx_executable(ctx, constructor_params)

        if stripped:
            unstripped_binary = cxx_output.unstripped_binary
        else:
            unstripped_binary = cxx_output.binary
        cxx_output.sub_targets["unstripped"] = [DefaultInfo(default_output = unstripped_binary)]

        dsym_artifact = get_apple_dsym(
            ctx = ctx,
            executable = unstripped_binary,
            debug_info = cxx_output.external_debug_info_artifacts,
            action_identifier = unstripped_binary.short_path,
        )
        cxx_output.sub_targets[DSYM_SUBTARGET] = [DefaultInfo(default_output = dsym_artifact)]

        min_version = get_min_deployment_version_for_node(ctx)
        min_version_providers = [AppleMinDeploymentVersionInfo(version = min_version)]

        non_exported_deps = cxx_attr_deps(ctx)
        exported_deps = cxx_attr_exported_deps(ctx)
        resource_graph = create_resource_graph(
            ctx = ctx,
            labels = ctx.attrs.labels,
            deps = non_exported_deps,
            exported_deps = exported_deps,
        )
        bundle_infos = get_bundle_infos_from_graph(resource_graph)
        if cxx_output.linker_map_data:
            bundle_infos.append(AppleBundleLinkerMapInfo(linker_maps = [cxx_output.linker_map_data.map]))

        link_command_providers = []
        if cxx_output.link_command_debug_output:
            link_command_providers.append(LinkCommandDebugOutputInfo(debug_outputs = [cxx_output.link_command_debug_output]))

        sanitizer_runtime_providers = []
        if cxx_output.sanitizer_runtime_files:
            sanitizer_runtime_providers.append(CxxSanitizerRuntimeInfo(runtime_files = cxx_output.sanitizer_runtime_files))

        index_stores = []
        swift_index_stores = []

        if swift_compile:
            swift_index_stores.append(swift_compile.index_store)
            index_stores.append(swift_compile.index_store)

        index_stores.extend(cxx_output.index_stores)

        all_deps = non_exported_deps + exported_deps

        xcassets_usage_providers, xcassets_usage_subtargets = xcassets_asset_symbol_usage_providers_and_subtargets(
            ctx, cxx_srcs, swift_srcs, always_provide = True
        )
        cxx_output.sub_targets.update(xcassets_usage_subtargets)

        index_store_subtargets, index_store_info = create_index_store_subtargets_and_provider(ctx, index_stores, swift_index_stores, all_deps)
        cxx_output.sub_targets.update(index_store_subtargets)

        # Modularization dependency graphs. Unlike apple_library (which routes
        # through cxx_library), binaries go through cxx_executable, which does
        # not create this provider -- so build it here. Without it the tset is
        # severed at the binary and apple_bundle collects nothing from the
        # libraries beneath it.
        mod_dep_graph_subtargets, mod_dep_graph_info = create_modularization_dep_graph_subtargets_and_provider(
            ctx,
            swift_compile.modularization_dependency_graph if swift_compile else None,
            all_deps,
        )
        cxx_output.sub_targets.update(mod_dep_graph_subtargets)

        target_stats_providers = []
        if TARGET_STATS_ENABLED:
            target_stats_tools = get_cxx_toolchain_info(ctx).target_stats_tools
            if target_stats_tools != None:
                # Headers as well as srcs: file_cycles derives its ObjC import
                # edges from the header entries, so without them a binary can
                # never report a cycle.
                target_stats_srcs = {src.file.short_path: src.file for src in cxx_srcs + swift_srcs}
                target_stats_srcs.update({
                    target_stats_header_name(header): header.artifact
                    for header in cxx_attr_headers_list(ctx, ctx.attrs.headers, get_apple_cxx_headers_layout(ctx))
                })
                target_stats_providers, target_stats_subtargets = target_stats_providers_and_subtargets(
                    ctx,
                    tools = target_stats_tools,
                    srcs = target_stats_srcs,
                    deps = all_deps,
                    cycle_mode = CycleMode("file"),
                    module_name = module_name,
                    swift_dot = swift_compile.modularization_dependency_graph if swift_compile else None,
                )
                cxx_output.sub_targets.update(target_stats_subtargets)

        validation_providers = [ValidationInfo(validations = cxx_output.validation_specs)] if cxx_output.validation_specs else []

        all_diagnostics = []
        if swift_compile:
            all_diagnostics.append(swift_compile.typecheck_file)
        if cxx_output.diagnostics:
            all_diagnostics.append(cxx_output.diagnostics)

        diagnostics_providers = [
            cxx_transitive_diagnostics_combine(
                ctx = ctx,
                diagnostics = all_diagnostics,
                # TODO: We might be able to use all_deps, but, this changes the iteration order of deps.
                deps = exported_deps + non_exported_deps,
            ),
        ]

        providers = (
            filter(
                None,
                [
                    DefaultInfo(default_output = cxx_output.binary, sub_targets = cxx_output.sub_targets),
                    RunInfo(args = cmd_args(cxx_output.binary, hidden = cxx_output.runtime_files)),
                    AppleEntitlementsInfo(entitlements_file = ctx.attrs.entitlements_file),
                    AppleDebuggableInfo(dsyms = [dsym_artifact], binaries = [unstripped_binary], debug_info_tset = cxx_output.external_debug_info),
                    cxx_output.xcode_data,
                    cxx_output.compilation_db,
                    merge_bundle_linker_maps_info(bundle_infos),
                    UnstrippedLinkOutputInfo(artifact = unstripped_binary),
                    index_store_info,
                    mod_dep_graph_info,
                    swift_dependency_info,
                ],
            )
            + [resource_graph]
            + min_version_providers
            + link_command_providers
            + sanitizer_runtime_providers
            + validation_providers
            + diagnostics_providers
            + target_stats_providers
            + xcassets_usage_providers
        )

        if cxx_output.xplugins_debug_artifacts_info:
            providers.append(cxx_output.xplugins_debug_artifacts_info)

        return providers

    if uses_explicit_modules(ctx):
        return get_swift_anonymous_targets(ctx, get_apple_binary_providers)
    else:
        return get_apple_binary_providers([])

def _get_extra_linker_outputs(
    ctx: AnalysisContext, extra_linker_output_category: ExtraLinkerOutputCategory = ExtraLinkerOutputCategory("produced-during-local-link")
) -> ExtraLinkerOutputs:
    _ = ctx  # buildifier: disable=unused-variable
    # @oss-disable[end= ]: return get_extra_linker_outputs(ctx, extra_linker_output_category)
    return ExtraLinkerOutputs() # @oss-enable

def _get_extra_linker_outputs_flags(
    ctx: AnalysisContext,
    outputs: dict[str, Artifact],
    extra_linker_output_category: ExtraLinkerOutputCategory = ExtraLinkerOutputCategory("produced-during-local-link"),
) -> list[ArgLike]:
    _ = ctx  # buildifier: disable=unused-variable
    # @oss-disable[end= ]: return get_extra_linker_output_flags(ctx, outputs, extra_linker_output_category)
    return [] # @oss-enable

def _extra_distributed_thin_lto_opt_outputs_merger(ctx: AnalysisContext, outputs_to_bind: dict[str, Artifact], outputs_to_merge: list[dict[str, Artifact]]):
    # @oss-disable[end= ]: return extra_distributed_thin_lto_opt_outputs_merger(ctx, outputs_to_bind, outputs_to_merge)
    return # @oss-enable

def _filter_swift_srcs(ctx: AnalysisContext) -> (list[CxxSrcWithFlags], list[CxxSrcWithFlags]):
    cxx_srcs = []
    swift_srcs = []
    for s in get_srcs_with_flags(ctx):
        if s.file.extension == SWIFT_EXTENSION:
            swift_srcs.append(s)
        else:
            cxx_srcs.append(s)
    return cxx_srcs, swift_srcs

def _get_bridging_header_flags(ctx: AnalysisContext) -> list[ArgLike]:
    if ctx.attrs.bridging_header:
        objc_bridging_header_flags = [
            # Disable bridging header -> PCH compilation to mitigate an issue in Xcode 13 beta.
            "-disable-bridging-pch",
            "-import-objc-header",
            cmd_args(ctx.attrs.bridging_header),
        ]

        headers_layout = cxx_get_regular_cxx_headers_layout(ctx)
        headers = cxx_attr_headers(ctx, headers_layout)
        header_map = {paths.join(h.namespace, h.name): h.artifact for h in headers}

        # We need to expose private headers to swift-compile action, in case something is imported to bridging header.
        cxx_toolchain_info = get_cxx_toolchain_info(ctx)
        header_mode = map_val(HeaderMode, getattr(ctx.attrs, "header_mode", None))
        allow_cache_upload = cxx_attrs_get_allow_cache_upload(ctx.attrs)
        uses_content_based_paths = get_uses_content_based_paths(ctx)
        header_root = prepare_headers(
            ctx.actions,
            cxx_toolchain_info,
            header_map,
            "apple-binary-private-headers",
            header_mode = header_mode,
            allow_cache_upload = allow_cache_upload,
            uses_content_based_paths = uses_content_based_paths,
        )
        if header_root != None:
            private_headers_args = [cmd_args("-I"), header_root.include_path]
        else:
            private_headers_args = []

        return objc_bridging_header_flags + private_headers_args
    else:
        return []
