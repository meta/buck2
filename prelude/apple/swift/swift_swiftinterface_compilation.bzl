# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolsInfo")
load("@prelude//apple:apple_utility.bzl", "expand_relative_prefixed_sdk_path")
load("@prelude//apple/swift:swift_pcm_compilation.bzl", "get_compiled_pcm_deps_tset")
load("@prelude//apple/swift:swift_types.bzl", "SWIFTMODULE_EXTENSION")
load(":apple_sdk_modules_utility.bzl", "get_compiled_sdk_clang_deps_tset", "get_compiled_sdk_swift_deps_tset")
load(
    ":swift_debug_info_utils.bzl",
    "extract_and_merge_clang_debug_infos",
    "extract_and_merge_swift_debug_infos",
)
load(
    ":swift_incremental_support.bzl",
    "get_uses_content_based_paths",
)
load(":swift_module_map.bzl", "write_swift_module_map_with_deps")
load(":swift_sdk_flags.bzl", "get_sdk_flags")
load(":swift_sdk_pcm_compilation.bzl", "get_swift_sdk_pcm_anon_targets")
load(":swift_toolchain.bzl", "get_swift_toolchain_info", "get_swift_toolchain_info_dep")
load(
    ":swift_toolchain_types.bzl",
    "SdkUncompiledModuleInfo",
    "SwiftCompiledModuleInfo",
    "SwiftCompiledModuleTset",
    "SwiftToolchainInfo",
    "WrappedSdkCompiledModuleInfo",
)

def get_swift_interface_anon_targets(ctx: AnalysisContext, uncompiled_sdk_deps: list[Dependency]):
    return [
        (
            _swift_interface_compilation,
            {
                # There's an "implicit" arg being passed - the execution platform
                # of the rule that's requesting the anon-targets. In this case,
                # callers of `get_swift_interface_anon_targets()` will call
                # `ctx.actions.anon_targets()`, so the exec platform will get
                # inherited.
                #
                # This has an important implication - the exact same SDK target
                # might get duplicated if it's requested `apple_library()`
                # targets which have different execution platforms, even if
                # everything else is the same - including the same toolchain.
                #
                # Because Swift compilation aggregates SDK modules from deps
                # and SDK modules from the target itself, a duplication of the
                # same compiled SDK module can occur in the module map.
                "dep": d,
                "has_content_based_path": True,
                "name": d.label,
                "_apple_tools": ctx.attrs._apple_tools,
                "_swift_toolchain": get_swift_toolchain_info_dep(ctx),
            },
        )
        for d in uncompiled_sdk_deps
        if d[SdkUncompiledModuleInfo].is_swiftmodule
    ]

def compile_swiftinterface_common(
    ctx,
    deps,
    is_framework,
    uncompiled_module_info_name,
    partial_cmd,
    sdk_deps_providers,
    expanded_swiftinterface_cmd,
    category,
    additional_compiled_pcm,
    additional_compiled_swiftmodules = None,
):
    uses_content_based_paths = get_uses_content_based_paths(ctx)
    swift_toolchain = get_swift_toolchain_info(ctx)
    cmd = cmd_args(swift_toolchain.compiler)
    cmd.add(partial_cmd)
    cmd.add(get_sdk_flags(ctx))

    if swift_toolchain.resource_dir:
        cmd.add([
            "-resource-dir",
            swift_toolchain.resource_dir,
        ])

    pcm_deps_tset = get_compiled_pcm_deps_tset(ctx, sdk_deps_providers)

    if additional_compiled_pcm:
        pcm_deps_tset = ctx.actions.tset(SwiftCompiledModuleTset, value = additional_compiled_pcm, children = [pcm_deps_tset])

    clang_deps_tset = get_compiled_sdk_clang_deps_tset(ctx, sdk_deps_providers)
    swift_deps_tset = get_compiled_sdk_swift_deps_tset(ctx, sdk_deps_providers + deps)

    #'additional_compiled_swiftmodules' in practice will be third-party frameworks
    if additional_compiled_swiftmodules:
        swift_deps_tset = ctx.actions.tset(SwiftCompiledModuleTset, children = [swift_deps_tset, additional_compiled_swiftmodules])

    all_deps_tset = ctx.actions.tset(
        SwiftCompiledModuleTset,
        children = [pcm_deps_tset, clang_deps_tset, swift_deps_tset],
    )

    _, swift_module_map_args = write_swift_module_map_with_deps(ctx, uncompiled_module_info_name, all_deps_tset)
    cmd.add([
        "-explicit-swift-module-map-file",
        swift_module_map_args,
    ])

    swiftmodule_output = ctx.actions.declare_output(uncompiled_module_info_name + SWIFTMODULE_EXTENSION, has_content_based_path = uses_content_based_paths)
    cmd.add([
        "-o",
        swiftmodule_output.as_output(),
        expanded_swiftinterface_cmd,
    ])

    ctx.actions.run(
        cmd,
        category = category,
        identifier = uncompiled_module_info_name,
    )

    return SwiftCompiledModuleInfo(
        is_framework = is_framework,
        is_sdk_module = True,
        is_swiftmodule = True,
        module_name = uncompiled_module_info_name,
        output_artifact = swiftmodule_output,
    ), swift_deps_tset

def _swift_interface_compilation_impl(ctx: AnalysisContext) -> [Promise, list[Provider]]:
    def k(sdk_deps_providers) -> list[Provider]:
        uncompiled_sdk_module_info = ctx.attrs.dep[SdkUncompiledModuleInfo]
        uncompiled_module_info_name = uncompiled_sdk_module_info.module_name
        swift_toolchain = ctx.attrs._swift_toolchain[SwiftToolchainInfo]

        expanded_swiftinterface_cmd = expand_relative_prefixed_sdk_path(
            swift_toolchain,
            uncompiled_sdk_module_info.input_relative_path,
        )

        compiled_sdk, swift_deps_tset = compile_swiftinterface_common(
            ctx,
            [],
            uncompiled_sdk_module_info.is_framework,
            uncompiled_module_info_name,
            uncompiled_sdk_module_info.partial_cmd,
            sdk_deps_providers,
            expanded_swiftinterface_cmd,
            "sdk_swiftinterface_compile",
            None,
        )

        wrapped_sdk_compiled_module_info = WrappedSdkCompiledModuleInfo(
            swift_deps = ctx.actions.tset(SwiftCompiledModuleTset, value = compiled_sdk, children = [swift_deps_tset]),
            swift_debug_info = extract_and_merge_swift_debug_infos(ctx, sdk_deps_providers, [compiled_sdk.output_artifact]),
            clang_debug_info = extract_and_merge_clang_debug_infos(ctx, sdk_deps_providers),
        )

        return [
            DefaultInfo(),
            wrapped_sdk_compiled_module_info,
        ]

    # For each swiftinterface compile its transitive clang deps with the provided target.
    module_info = ctx.attrs.dep[SdkUncompiledModuleInfo]
    clang_module_deps = get_swift_sdk_pcm_anon_targets(
        ctx,
        False,
        module_info.deps,
        ["-target", module_info.target],
    )

    # Compile the transitive swiftmodule deps.
    swift_module_deps = get_swift_interface_anon_targets(ctx, module_info.deps)

    return ctx.actions.anon_targets(clang_module_deps + swift_module_deps).promise.map(k)

_swift_interface_compilation = rule(
    impl = _swift_interface_compilation_impl,
    attrs = {
        "dep": attrs.dep(),
        "has_content_based_path": attrs.bool(),
        "_apple_tools": attrs.dep(providers = [AppleToolsInfo]),
        "_swift_toolchain": attrs.dep(),
    },
)
