# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//apple/swift:swift_helpers.bzl", "compile_with_argsfile")
load(
    ":apple_sdk_modules_utility.bzl",
    "get_compiled_sdk_clang_deps_tset",
    "get_uncompiled_sdk_deps",
)
load(
    ":swift_debug_info_utils.bzl",
    "extract_and_merge_clang_debug_infos",
)
load(":swift_pcm_compilation_types.bzl", "SwiftPCMUncompiledInfo", "WrappedSwiftPCMCompiledInfo")
load(":swift_sdk_flags.bzl", "get_sdk_flags")
load(":swift_sdk_pcm_compilation.bzl", "get_shared_pcm_compilation_args", "get_swift_sdk_pcm_anon_targets")
load(":swift_toolchain.bzl", "get_swift_toolchain_info", "get_swift_toolchain_info_dep")
load(":swift_toolchain_types.bzl", "SwiftCompiledModuleInfo", "SwiftCompiledModuleTset", "SwiftToolchainInfo", "WrappedSdkCompiledModuleInfo")

_REQUIRED_SDK_MODULES = ["Foundation"]

_REQUIRED_SDK_CXX_MODULES = _REQUIRED_SDK_MODULES + ["std"]

def get_compiled_pcm_deps_tset(ctx: AnalysisContext, pcm_deps_providers: list) -> SwiftCompiledModuleTset:
    pcm_deps = [
        pcm_deps_provider[WrappedSwiftPCMCompiledInfo].clang_deps
        for pcm_deps_provider in pcm_deps_providers
        if WrappedSwiftPCMCompiledInfo in pcm_deps_provider
    ]
    return ctx.actions.tset(SwiftCompiledModuleTset, children = pcm_deps)

def get_swift_pcm_anon_targets(ctx: AnalysisContext, uncompiled_deps: list[Dependency], swift_cxx_args: list[str], enable_cxx_interop: bool):
    deps = []
    for uncompiled_dep in uncompiled_deps:
        if SwiftPCMUncompiledInfo not in uncompiled_dep:
            continue

        # T209485965: workaround for depagg to avoid duplicate clang modules
        # when traversing deps through the base target and [headers] subtarget.
        # By always requesting the [headers] subtarget we should use the same
        # anon actions for both paths.
        if "headers" in uncompiled_dep[DefaultInfo].sub_targets:
            uncompiled_dep = uncompiled_dep.sub_target("headers")

        deps.append(
            (
                _swift_pcm_compilation,
                {
                    "dep": uncompiled_dep,
                    "enable_cxx_interop": enable_cxx_interop,
                    "has_content_based_path": False,
                    "name": uncompiled_dep.label,
                    "swift_cxx_args": swift_cxx_args,
                    "_swift_toolchain": get_swift_toolchain_info_dep(ctx),
                },
            )
        )

    return deps

def _compiled_module_info(module_name: str, pcm_output: Artifact, pcm_info: SwiftPCMUncompiledInfo) -> SwiftCompiledModuleInfo:
    return SwiftCompiledModuleInfo(
        clang_importer_args = pcm_info.exported_clang_importer_args,
        clang_modulemap_path = cmd_args(pcm_info.modulemap_artifact),
        is_framework = False,
        is_sdk_module = False,
        is_swiftmodule = False,
        module_name = module_name,
        output_artifact = pcm_output,
    )

def _swift_pcm_compilation_impl(ctx: AnalysisContext) -> [Promise, list[Provider]]:
    def k(compiled_pcm_deps_providers) -> list[Provider]:
        uncompiled_pcm_info = ctx.attrs.dep[SwiftPCMUncompiledInfo]

        # `compiled_pcm_deps_providers` will contain `WrappedSdkCompiledModuleInfo` providers
        # from direct SDK deps and transitive deps that export sdk deps.
        sdk_deps_tset = get_compiled_sdk_clang_deps_tset(ctx, compiled_pcm_deps_providers)

        # To compile a pcm we only use the exported_deps as those are the only
        # ones that should be transitively exported through public headers
        pcm_deps_tset = get_compiled_pcm_deps_tset(ctx, compiled_pcm_deps_providers)

        # We don't need to compile non-modular or targets that do not export any headers,
        # but for the sake of BUCK1 compatibility, we need to pass them up,
        # in case they re-export some dependencies.
        if uncompiled_pcm_info.is_transient:
            return [
                DefaultInfo(),
                WrappedSwiftPCMCompiledInfo(
                    clang_deps = pcm_deps_tset,
                    clang_debug_info = extract_and_merge_clang_debug_infos(ctx, compiled_pcm_deps_providers),
                ),
                WrappedSdkCompiledModuleInfo(
                    clang_deps = sdk_deps_tset,
                    clang_debug_info = extract_and_merge_clang_debug_infos(ctx, compiled_pcm_deps_providers),
                ),
            ]

        module_name = uncompiled_pcm_info.name
        swift_toolchain_info = ctx.attrs._swift_toolchain[SwiftToolchainInfo]
        cmd, additional_cmd, pcm_output = _get_base_pcm_flags(
            ctx,
            module_name,
            swift_toolchain_info,
            uncompiled_pcm_info,
            sdk_deps_tset,
            pcm_deps_tset,
            ctx.attrs.swift_cxx_args,
        )
        additional_cmd.add(uncompiled_pcm_info.modulemap_artifact)

        compile_with_argsfile(
            ctx,
            category = "swift_pcm_compile",
            shared_flags = cmd,
            srcs = [],
            additional_flags = additional_cmd,
            toolchain = swift_toolchain_info,
            supports_output_file_map = False,
        )
        pcm_info = _compiled_module_info(module_name, pcm_output, uncompiled_pcm_info)
        debug_artifacts = [
            pcm_info.output_artifact,
            uncompiled_pcm_info.modulemap_artifact,
        ]

        return [
            DefaultInfo(default_outputs = [pcm_output]),
            WrappedSwiftPCMCompiledInfo(
                clang_deps = ctx.actions.tset(SwiftCompiledModuleTset, value = pcm_info, children = [pcm_deps_tset]),
                clang_debug_info = extract_and_merge_clang_debug_infos(ctx, compiled_pcm_deps_providers, debug_artifacts),
            ),
            WrappedSdkCompiledModuleInfo(
                clang_deps = sdk_deps_tset,
                # No need to further propagate debug info, it will be included
                # in WrappedSwiftPCMCompiledInfo.
            ),
        ]

    required_sdk_modules = _REQUIRED_SDK_CXX_MODULES if ctx.attrs.enable_cxx_interop else _REQUIRED_SDK_MODULES
    direct_uncompiled_sdk_deps = get_uncompiled_sdk_deps(
        ctx.attrs.dep[SwiftPCMUncompiledInfo].uncompiled_sdk_modules,
        required_sdk_modules,
        ctx.attrs._swift_toolchain[SwiftToolchainInfo],
    )

    # Recursively compiling SDK's Clang dependencies
    sdk_pcm_deps_anon_targets = get_swift_sdk_pcm_anon_targets(
        ctx,
        ctx.attrs.enable_cxx_interop,
        direct_uncompiled_sdk_deps,
        ctx.attrs.swift_cxx_args,
    )

    # Recursively compile PCMs of transitevely visible exported_deps
    swift_pcm_anon_targets = get_swift_pcm_anon_targets(
        ctx,
        ctx.attrs.dep[SwiftPCMUncompiledInfo].exported_deps,
        ctx.attrs.swift_cxx_args,
        ctx.attrs.enable_cxx_interop,
    )
    return ctx.actions.anon_targets(sdk_pcm_deps_anon_targets + swift_pcm_anon_targets).promise.map(k)

_swift_pcm_compilation = rule(
    impl = _swift_pcm_compilation_impl,
    attrs = {
        "dep": attrs.dep(),
        "enable_cxx_interop": attrs.bool(),
        "has_content_based_path": attrs.bool(),
        "swift_cxx_args": attrs.list(attrs.string(), default = []),
        "_swift_toolchain": attrs.dep(),
    },
)

def _compile_clang_headers(
    ctx: AnalysisContext,
    action_name: str,
    module_name: str,
    uncompiled_pcm_info: SwiftPCMUncompiledInfo,
    compiled_pcm_deps_providers,
    swift_cxx_args: list[str],
    additional_args: cmd_args,
    header: Artifact | None = None,
) -> Artifact:
    # `compiled_pcm_deps_providers` will contain `WrappedSdkCompiledModuleInfo` providers
    # from direct SDK deps and transitive deps that export sdk deps.
    sdk_deps_tset = get_compiled_sdk_clang_deps_tset(ctx, compiled_pcm_deps_providers)

    # To compile a pcm we only use the exported_deps as those are the only
    # ones that should be transitively exported through public headers
    pcm_deps_tset = get_compiled_pcm_deps_tset(ctx, compiled_pcm_deps_providers)

    swift_toolchain_info = get_swift_toolchain_info(ctx)
    cmd, additional_cmd, pcm_output = _get_base_pcm_flags(
        ctx,
        module_name,
        swift_toolchain_info,
        uncompiled_pcm_info,
        sdk_deps_tset,
        pcm_deps_tset,
        swift_cxx_args,
        is_pch = header != None,
    )
    additional_cmd.add(header if header != None else uncompiled_pcm_info.modulemap_artifact)
    cmd.add(additional_args)
    compile_with_argsfile(
        ctx,
        category = action_name,
        shared_flags = cmd,
        srcs = [],
        additional_flags = additional_cmd,
        toolchain = swift_toolchain_info,
        supports_output_file_map = False,
    )
    return pcm_output

def _compile_pcm(
    ctx: AnalysisContext,
    action_name: str,
    module_name: str,
    uncompiled_pcm_info: SwiftPCMUncompiledInfo,
    compiled_pcm_deps_providers,
    swift_cxx_args: list[str],
    additional_args: cmd_args,
) -> SwiftCompiledModuleInfo:
    pcm_output = _compile_clang_headers(
        ctx,
        action_name,
        module_name,
        uncompiled_pcm_info,
        compiled_pcm_deps_providers,
        swift_cxx_args,
        additional_args,
    )
    return _compiled_module_info(module_name, pcm_output, uncompiled_pcm_info)

def compile_internal_bridging_header_pch(
    ctx: AnalysisContext,
    module_name: str,
    header: Artifact,
    uncompiled_pcm_info: SwiftPCMUncompiledInfo,
    compiled_pcm_deps_providers,
    swift_cxx_args: list[str],
    additional_args: cmd_args,
) -> Artifact:
    # Embed inputs so toolchains without cwd-relative PCH paths can still
    # load a PCH built on another machine.
    additional_args = cmd_args(additional_args, "-Xcc", "-Xclang", "-Xcc", "-fmodules-embed-all-files")
    return _compile_clang_headers(
        ctx,
        "swift_internal_bridging_header_pch_compile",
        module_name,
        uncompiled_pcm_info,
        compiled_pcm_deps_providers,
        swift_cxx_args,
        additional_args,
        header = header,
    )

def compile_framework_pcm(
    ctx: AnalysisContext, module_name: str, uncompiled_pcm_info: SwiftPCMUncompiledInfo, compiled_pcm_deps_providers, swift_cxx_args: list[str]
) -> SwiftCompiledModuleInfo:
    return _compile_pcm(
        ctx,
        "swift_prebuilt_framework_pcm_compile",
        module_name,
        uncompiled_pcm_info,
        compiled_pcm_deps_providers,
        swift_cxx_args,
        cmd_args(),
    )

def compile_underlying_pcm(
    ctx: AnalysisContext,
    module_name: str,
    uncompiled_pcm_info: SwiftPCMUncompiledInfo,
    compiled_pcm_deps_providers,
    swift_cxx_args: list[str],
    framework_search_path_flags: cmd_args,
) -> SwiftCompiledModuleInfo:
    return _compile_pcm(
        ctx,
        "swift_underlying_pcm_compile",
        module_name,
        uncompiled_pcm_info,
        compiled_pcm_deps_providers,
        swift_cxx_args,
        framework_search_path_flags,
    )

def _get_base_pcm_flags(
    ctx: AnalysisContext,
    module_name: str,
    swift_toolchain_info: SwiftToolchainInfo,
    uncompiled_pcm_info: SwiftPCMUncompiledInfo,
    sdk_deps_tset: SwiftCompiledModuleTset,
    pcm_deps_tset: SwiftCompiledModuleTset,
    swift_cxx_args: list[str],
    is_pch: bool = False,
) -> (cmd_args, cmd_args, Artifact):
    uses_content_based_paths = False
    output_extension = ".pch" if is_pch else ".pcm"
    pcm_output = ctx.actions.declare_output(module_name + output_extension, has_content_based_path = uses_content_based_paths)
    cmd = cmd_args(
        get_shared_pcm_compilation_args(module_name),
        get_sdk_flags(ctx),
        swift_toolchain_info.compiler_flags,
        (
            [
                "-resource-dir",
                swift_toolchain_info.resource_dir,
            ]
            if swift_toolchain_info.resource_dir
            else []
        ),
        sdk_deps_tset.project_as_args("clang_module_file_flags"),
        pcm_deps_tset.project_as_args("clang_module_file_flags"),
        pcm_deps_tset.project_as_args("clang_importer_flags"),
        # Order is important here, we need to add the symlink tree search paths
        # before the headermaps to avoid mixing include paths.
        uncompiled_pcm_info.exported_clang_importer_args,
        uncompiled_pcm_info.clang_importer_args,
    )

    cmd.add(swift_cxx_args)
    if is_pch:
        # The driver has no standalone PCH mode. Use its Clang-input mode and
        # override the frontend action to emit a PCH instead of a PCM.
        cmd.add("-Xfrontend", "-emit-pch", "-Xcc", "-Xclang", "-Xcc", "-fno-pch-timestamp")
        if swift_toolchain_info.supports_pch_base_dir_is_cwd:
            cmd.add("-Xcc", "-fpch-base-dir-is-cwd")

    additional_cmd = cmd_args("-o", pcm_output.as_output())

    return (cmd, additional_cmd, pcm_output)
