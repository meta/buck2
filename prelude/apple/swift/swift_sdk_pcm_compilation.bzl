# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//apple:apple_error_handler.bzl", "apple_build_error_handler")
load("@prelude//apple:apple_utility.bzl", "expand_relative_prefixed_sdk_path", "get_disable_pch_validation_flags")
load(":apple_sdk_modules_utility.bzl", "get_compiled_sdk_clang_deps_tset")
load(
    ":swift_debug_info_utils.bzl",
    "extract_and_merge_clang_debug_infos",
)
load(":swift_sdk_flags.bzl", "get_sdk_flags")
load(":swift_toolchain.bzl", "get_swift_toolchain_info_dep")
load(
    ":swift_toolchain_types.bzl",
    "SdkUncompiledModuleInfo",
    "SwiftCompiledModuleInfo",
    "SwiftCompiledModuleTset",
    "SwiftToolchainInfo",
    "WrappedSdkCompiledModuleInfo",
)

def get_shared_pcm_compilation_args(module_name: str) -> cmd_args:
    cmd = cmd_args()
    cmd.add([
        "-emit-pcm",
        "-module-name",
        module_name,
        "-Xfrontend",
        "-disable-implicit-swift-modules",
        "-Xcc",
        "-fno-implicit-modules",
        "-Xcc",
        "-fno-implicit-module-maps",
        # Set the base directory of the pcm file to the working directory, which ensures
        # all paths serialized in the PCM are relative.
        "-Xcc",
        "-Xclang",
        "-Xcc",
        "-fmodule-file-home-is-cwd",
        # We cannot set an empty Swift working directory as that would end up serializing
        # absolute header search paths in the PCM. Instead unset the clang working directory
        # to avoid serializing it as an absolute path.
        "-Xcc",
        "-working-directory=",
        # AssetsLibrary is shipping with a #warning, which we shouldn't error on when compiling
        # the SDK module. I don't think this is actually avoidable or removable until the next xcode major version
        "-Xcc",
        "-Wno-error=#warnings",
        # When compiling PCMs with the new Swift driver, we need to manually set -file-compilation-dir.
        # If not defined, the driver will append this path automatically,
        # resulting in the emission of absolute paths in the PCMs.
        "-file-compilation-dir",
        ".",
    ])

    cmd.add(get_disable_pch_validation_flags())

    return cmd

def _remove_path_components_from_right(path: str, count: int):
    path_components = path.split("/")
    removed_path = "/".join(path_components[0:-count])
    return removed_path

def _add_sdk_module_search_path(cmd, uncompiled_sdk_module_info, swift_toolchain_info):
    modulemap_path = uncompiled_sdk_module_info.input_relative_path

    # If this input is a framework we need to search above the
    # current framework location, otherwise we include the
    # modulemap root.
    if uncompiled_sdk_module_info.is_framework:
        frameworks_dir_path = _remove_path_components_from_right(modulemap_path, 3)
        expanded_path = expand_relative_prefixed_sdk_path(swift_toolchain_info, frameworks_dir_path)
    else:
        module_root_path = _remove_path_components_from_right(modulemap_path, 1)
        expanded_path = expand_relative_prefixed_sdk_path(swift_toolchain_info, module_root_path)
    cmd.add(
        "-Xcc",
        ("-F" if uncompiled_sdk_module_info.is_framework else "-I"),
        "-Xcc",
        expanded_path,
    )

def get_swift_sdk_pcm_anon_targets(ctx: AnalysisContext, enable_cxx_interop: bool, uncompiled_sdk_deps: list[Dependency], swift_cxx_args: list[str]):
    # We include the Swift deps here too as we need
    # to include their transitive clang deps.
    return [
        (
            _swift_sdk_pcm_compilation,
            {
                "dep": module_dep,
                "enable_cxx_interop": enable_cxx_interop,
                "has_content_based_path": False,
                "name": module_dep.label,
                "swift_cxx_args": swift_cxx_args,
                "_swift_toolchain": get_swift_toolchain_info_dep(ctx),
            },
        )
        for module_dep in uncompiled_sdk_deps
    ]

def _swift_sdk_pcm_compilation_impl(ctx: AnalysisContext) -> [Promise, list[Provider]]:
    def k(sdk_pcm_deps_providers) -> list[Provider]:
        uncompiled_sdk_module_info = ctx.attrs.dep[SdkUncompiledModuleInfo]
        sdk_deps_tset = get_compiled_sdk_clang_deps_tset(ctx, sdk_pcm_deps_providers)
        uses_content_based_paths = False

        # We pass in Swift and Clang SDK module deps to get the transitive
        # Clang dependencies compiled with the correct Swift cxx args. For
        # Swift modules we just want to pass up the clang deps.
        if uncompiled_sdk_module_info.is_swiftmodule:
            return [
                DefaultInfo(),
                WrappedSdkCompiledModuleInfo(
                    clang_deps = sdk_deps_tset,
                    clang_debug_info = extract_and_merge_clang_debug_infos(ctx, sdk_pcm_deps_providers),
                ),
            ]

        module_name = uncompiled_sdk_module_info.module_name
        swift_toolchain = ctx.attrs._swift_toolchain[SwiftToolchainInfo]
        cmd = cmd_args(swift_toolchain.compiler)
        argsfile_cmd = cmd_args(uncompiled_sdk_module_info.partial_cmd)
        argsfile_cmd.add(get_sdk_flags(ctx))
        argsfile_cmd.add(swift_toolchain.compiler_flags)

        if swift_toolchain.resource_dir:
            argsfile_cmd.add([
                "-resource-dir",
                swift_toolchain.resource_dir,
            ])

        if not swift_toolchain.supports_relative_resource_dir:
            # When the compiler does not correctly serialize builtin header paths
            # we need to specify the CWD as a search path to find the headers.
            argsfile_cmd.add([
                "-Xcc",
                "-I.",
            ])

        argsfile_cmd.add(sdk_deps_tset.project_as_args("clang_module_file_flags"))

        # For SDK modules we need to set a few more args
        argsfile_cmd.add([
            "-Xcc",
            "-Xclang",
            "-Xcc",
            "-emit-module",
            "-Xcc",
            "-Xclang",
            "-Xcc",
            "-fsystem-module",
        ])

        argsfile_cmd.add(ctx.attrs.swift_cxx_args)

        if ctx.attrs.enable_cxx_interop:
            # The stdlib headers have deprecation warnings set when targeting
            # more recent versions. These warnings get serialized in the
            # modules and make it impossible to import the std module, so
            # suppress them during compilation instead.
            argsfile_cmd.add([
                "-Xcc",
                "-D_LIBCPP_DISABLE_DEPRECATION_WARNINGS",
            ])

            if module_name == "Darwin":
                # The Darwin module requires special handling with cxx interop
                # to ensure that it does not include the c++ headers. The module
                # is marked with [no_undeclared_includes] which will prevent
                # including headers declared in other modulemaps. So that the
                # cxx modules are visible we need to pass the module map path
                # without the corresponding module file, which we cannot build
                # until the Darwin module is available.
                argsfile_cmd.add([
                    "-Xcc",
                    cmd_args(swift_toolchain.sdk_path, format = "-fmodule-map-file={}/usr/include/c++/v1/module.modulemap"),
                ])

        _add_sdk_module_search_path(argsfile_cmd, uncompiled_sdk_module_info, swift_toolchain)

        shell_quoted_args = cmd_args(argsfile_cmd, quote = "shell")
        argsfile, _ = ctx.actions.write("sdk_pcm_compile_argsfile", shell_quoted_args, allow_args = True, has_content_based_path = uses_content_based_paths)
        cmd.add(cmd_args(argsfile, format = "@{}", delimiter = ""))
        cmd.add(cmd_args(hidden = [argsfile_cmd]))

        expanded_modulemap_path_cmd = expand_relative_prefixed_sdk_path(
            swift_toolchain,
            uncompiled_sdk_module_info.input_relative_path,
        )
        pcm_output = ctx.actions.declare_output(module_name + ".pcm", has_content_based_path = uses_content_based_paths)
        cmd.add([
            "-o",
            pcm_output.as_output(),
            expanded_modulemap_path_cmd,
        ])

        ctx.actions.run(
            cmd,
            category = "sdk_swift_pcm_compile",
            identifier = module_name,
            # Swift compiler requires unique inodes for all input files.
            unique_input_inodes = True,
            error_handler = apple_build_error_handler,
        )

        compiled_sdk = SwiftCompiledModuleInfo(
            clang_modulemap_path = expanded_modulemap_path_cmd,
            is_framework = uncompiled_sdk_module_info.is_framework,
            is_sdk_module = True,
            is_swiftmodule = False,
            module_name = module_name,
            output_artifact = pcm_output,
        )

        return [
            DefaultInfo(),
            WrappedSdkCompiledModuleInfo(
                clang_deps = ctx.actions.tset(SwiftCompiledModuleTset, value = compiled_sdk, children = [sdk_deps_tset]),
                clang_debug_info = extract_and_merge_clang_debug_infos(ctx, sdk_pcm_deps_providers, [pcm_output]),
            ),
        ]

    # Compile the transitive clang module deps of this target.
    deps = ctx.attrs.dep[SdkUncompiledModuleInfo].cxx_deps if ctx.attrs.enable_cxx_interop else ctx.attrs.dep[SdkUncompiledModuleInfo].deps
    clang_module_deps = get_swift_sdk_pcm_anon_targets(
        ctx,
        ctx.attrs.enable_cxx_interop,
        deps,
        ctx.attrs.swift_cxx_args,
    )

    return ctx.actions.anon_targets(clang_module_deps).promise.map(k)

_swift_sdk_pcm_compilation = rule(
    impl = _swift_sdk_pcm_compilation_impl,
    attrs = {
        "dep": attrs.dep(),
        "enable_cxx_interop": attrs.bool(),
        "has_content_based_path": attrs.bool(),
        "swift_cxx_args": attrs.list(attrs.string(), default = []),
        "_swift_toolchain": attrs.dep(),
    },
)
