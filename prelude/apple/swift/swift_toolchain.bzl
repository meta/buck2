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
    "make_artifact_tset",
)
load(":swift_toolchain_types.bzl", "SdkUncompiledModuleInfo", "SwiftObjectFormat", "SwiftToolchainInfo")

def get_swift_toolchain_info(ctx: AnalysisContext) -> SwiftToolchainInfo:
    if hasattr(ctx.attrs, "_swift_toolchain"):
        return ctx.attrs._swift_toolchain[SwiftToolchainInfo]
    else:
        return ctx.attrs._apple_toolchain[SwiftToolchainInfo]

def get_swift_toolchain_info_dep(ctx: AnalysisContext) -> Dependency:
    if hasattr(ctx.attrs, "_swift_toolchain"):
        return ctx.attrs._swift_toolchain
    else:
        return ctx.attrs._apple_toolchain

def supports_modulemaps_with_hmaps(ctx: AnalysisContext) -> bool:
    return get_swift_toolchain_info(ctx).supports_modulemaps_with_hmaps

def include_path_for_relative_module_map_paths(ctx: AnalysisContext) -> list[str]:
    if supports_modulemaps_with_hmaps(ctx):
        # modulemap paths are relative to the modulemap file, but the compiler
        # needs search paths to be able to find the headers themselves. When
        # EmitClangHeaderWithNonModularIncludes is enabled, we need to be able
        # to resolve these paths back to something that is in a headermap. The
        # headermaps contain two forms:
        #  1. <ModuleName/HeaderName.h>
        #  2. <repo/relative/path/to/header.h>
        # We can use the latter in every case by adding the repo root as a
        # header search path for the ClangImporter when emitting ObjC headers.
        return ["-Xcc", "-I."]
    else:
        return []

def _traverse_sdk_modules_graph(
    swift_sdk_module_name_to_deps: dict[str, Dependency], clang_sdk_module_name_to_deps: dict[str, Dependency], sdk_module_dep: Dependency
):
    if SdkUncompiledModuleInfo not in sdk_module_dep:
        return

    uncompiled_sdk_module_info = sdk_module_dep[SdkUncompiledModuleInfo]

    # If input_relative_path is None then this module represents a root node of SDK modules graph.
    # In such case, we need to handle only its deps.
    if uncompiled_sdk_module_info.input_relative_path == None:
        for uncompiled_dep in uncompiled_sdk_module_info.deps:
            _traverse_sdk_modules_graph(swift_sdk_module_name_to_deps, clang_sdk_module_name_to_deps, uncompiled_dep)
        return

    # return if dep is already in dict
    if uncompiled_sdk_module_info.is_swiftmodule and uncompiled_sdk_module_info.module_name in swift_sdk_module_name_to_deps:
        return
    elif not uncompiled_sdk_module_info.is_swiftmodule and uncompiled_sdk_module_info.module_name in clang_sdk_module_name_to_deps:
        return

    for uncompiled_dep in uncompiled_sdk_module_info.deps + uncompiled_sdk_module_info.cxx_deps:
        _traverse_sdk_modules_graph(swift_sdk_module_name_to_deps, clang_sdk_module_name_to_deps, uncompiled_dep)

    if uncompiled_sdk_module_info.is_swiftmodule:
        swift_sdk_module_name_to_deps[uncompiled_sdk_module_info.module_name] = sdk_module_dep
    else:
        clang_sdk_module_name_to_deps[uncompiled_sdk_module_info.module_name] = sdk_module_dep

def compute_sdk_module_graph(sdk_module_deps: list[Dependency]) -> (dict[str, Dependency], dict[str, Dependency]):
    # All Clang's PCMs need to be compiled with cxx flags of the target that imports them,
    # because of that, we expose `dependency`s of SDK modules,
    # which might be accessed from apple_library/apple_test rules and compiled there.
    uncompiled_swift_sdk_modules_deps = {}
    uncompiled_clang_sdk_modules_deps = {}

    for sdk_module_dep in sdk_module_deps:
        _traverse_sdk_modules_graph(
            uncompiled_swift_sdk_modules_deps,
            uncompiled_clang_sdk_modules_deps,
            sdk_module_dep,
        )

    return uncompiled_swift_sdk_modules_deps, uncompiled_clang_sdk_modules_deps

def swift_toolchain_impl(ctx):
    uncompiled_swift_sdk_modules_deps, uncompiled_clang_sdk_modules_deps = compute_sdk_module_graph(ctx.attrs.sdk_modules)

    # Module map files can live in the SDK or the toolchain resource dir.
    # We need to pass through both to ensure the debuginfo target
    # materializes them.
    debug_info_artifacts = []
    if ctx.attrs.sdk_path:
        debug_info_artifacts.append(ctx.attrs.sdk_path)
    if ctx.attrs.resource_dir:
        debug_info_artifacts.append(ctx.attrs.resource_dir)

    sdk_debug_info = make_artifact_tset(
        actions = ctx.actions,
        label = ctx.label,
        artifacts = debug_info_artifacts,
        tags = [ArtifactInfoTag("swift_debug_info")],
    )

    return [
        DefaultInfo(),
        SwiftToolchainInfo(
            architecture = ctx.attrs.architecture,
            compiler = cmd_args(ctx.attrs._swiftc_wrapper[RunInfo]).add(ctx.attrs.swiftc[RunInfo]),
            compiler_flags = ctx.attrs.swiftc_flags,
            mk_swift_comp_db = ctx.attrs.make_swift_comp_db[RunInfo],
            mk_swift_interface = cmd_args(ctx.attrs._swiftc_wrapper[RunInfo]).add(ctx.attrs.make_swift_interface[RunInfo]),
            object_format = SwiftObjectFormat(ctx.attrs.object_format) if ctx.attrs.object_format else SwiftObjectFormat("object"),
            platform_path = ctx.attrs.platform_path,
            prioritize_swift_critical_path = ctx.attrs.prioritize_swift_critical_path,
            provide_swift_debug_info = ctx.attrs.provide_swift_debug_info,
            resource_dir = ctx.attrs.resource_dir,
            sdk_module_path_prefixes = ctx.attrs.sdk_module_path_prefixes,
            sdk_path = ctx.attrs._internal_sdk_path or ctx.attrs.sdk_path,
            sdk_debug_info = sdk_debug_info,
            serialized_diags_to_json = ctx.attrs.serialized_diags_to_json[RunInfo] if ctx.attrs.serialized_diags_to_json else None,
            supports_explicit_module_debug_serialization = ctx.attrs.supports_explicit_module_debug_serialization,
            supports_incremental_file_hashing = ctx.attrs.supports_incremental_file_hashing,
            supports_modularization_dependency_graph = ctx.attrs.supports_modularization_dependency_graph,
            supports_modulemaps_with_hmaps = ctx.attrs.supports_modulemaps_with_hmaps,
            supports_pch_base_dir_is_cwd = ctx.attrs.supports_pch_base_dir_is_cwd,
            supports_relative_resource_dir = ctx.attrs.supports_relative_resource_dir,
            swift_ide_test_tool = ctx.attrs.swift_ide_test_tool[RunInfo] if ctx.attrs.swift_ide_test_tool else None,
            swift_stdlib_tool = ctx.attrs.swift_stdlib_tool[RunInfo],
            swift_stdlib_tool_flags = ctx.attrs.swift_stdlib_tool_flags,
            swift_experimental_features = ctx.attrs.swift_experimental_features,
            swift_upcoming_features = ctx.attrs.swift_upcoming_features,
            uncompiled_clang_sdk_modules_deps = uncompiled_clang_sdk_modules_deps,
            uncompiled_swift_sdk_modules_deps = uncompiled_swift_sdk_modules_deps,
            enforce_dedupe_eligibility = ctx.attrs.enforce_dedupe_eligibility,
            use_depsfiles = ctx.attrs.use_depsfiles,
            uses_content_based_paths = ctx.attrs.uses_content_based_paths,
        ),
    ]
