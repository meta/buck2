# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    ":swift_toolchain_types.bzl",
    "SdkSwiftOverlayInfo",
    "SdkUncompiledModuleInfo",
    "SwiftCompiledModuleTset",
    "SwiftToolchainInfo",  # @unused Used as a type
    "WrappedSdkCompiledModuleInfo",
)

def is_sdk_modules_provided(toolchain: SwiftToolchainInfo) -> bool:
    has_swift_modules = bool(toolchain.uncompiled_swift_sdk_modules_deps)
    has_clang_modules = bool(toolchain.uncompiled_clang_sdk_modules_deps)
    return has_swift_modules or has_clang_modules

def get_compiled_sdk_clang_deps_tset(ctx: AnalysisContext, deps_providers: list) -> SwiftCompiledModuleTset:
    sdk_deps = [
        d[WrappedSdkCompiledModuleInfo].clang_deps
        for d in deps_providers
        if WrappedSdkCompiledModuleInfo in d and d[WrappedSdkCompiledModuleInfo].clang_deps != None
    ]
    return ctx.actions.tset(SwiftCompiledModuleTset, children = sdk_deps)

def get_compiled_sdk_swift_deps_tset(ctx: AnalysisContext, deps_providers: list) -> SwiftCompiledModuleTset:
    sdk_deps = [
        d[WrappedSdkCompiledModuleInfo].swift_deps
        for d in deps_providers
        if WrappedSdkCompiledModuleInfo in d and d[WrappedSdkCompiledModuleInfo].swift_deps != None
    ]
    return ctx.actions.tset(SwiftCompiledModuleTset, children = sdk_deps)

def get_uncompiled_sdk_deps(sdk_modules: list[str], required_modules: list[str], toolchain: SwiftToolchainInfo) -> list[Dependency]:
    if not is_sdk_modules_provided(toolchain):
        fail("SDK deps are not set for swift_toolchain")

    swift_map = toolchain.uncompiled_swift_sdk_modules_deps
    clang_map = toolchain.uncompiled_clang_sdk_modules_deps

    direct_names = set(sdk_modules + required_modules)

    sdk_deps = []
    for sdk_module_dep_name in list(direct_names):
        if sdk_module_dep_name in swift_map:
            sdk_deps.append(swift_map[sdk_module_dep_name])
        if sdk_module_dep_name in clang_map:
            sdk_deps.append(clang_map[sdk_module_dep_name])

    # ModuleA declares a cross-import overlay on ModuleADependency, which the
    # compiler auto-loads whenever both are visible, even if ModuleADependency
    # only arrives transitively. Match overlays against the precomputed
    # overlay-filtered transitive closure, not just the direct sdk_modules.
    reachable = set(direct_names)
    for sdk_module_dep_name in list(direct_names):
        for uncompiled_sdk_modules_map in (swift_map, clang_map):
            if sdk_module_dep_name not in uncompiled_sdk_modules_map:
                continue
            sdk_dep = uncompiled_sdk_modules_map[sdk_module_dep_name]
            if SdkUncompiledModuleInfo not in sdk_dep:
                continue
            transitive_names = sdk_dep[SdkUncompiledModuleInfo].overlays_transitive_deps or []
            for transitive_name in transitive_names:
                reachable.add(transitive_name)

    sdk_overlays = []
    # Seed with the direct deps: an overlay listed explicitly in sdk_modules
    # is already in sdk_deps above and must not be added a second time.
    seen_overlays = set(direct_names)
    for reachable_name in list(reachable):
        for uncompiled_sdk_modules_map in (swift_map, clang_map):
            if reachable_name not in uncompiled_sdk_modules_map:
                continue
            sdk_dep = uncompiled_sdk_modules_map[reachable_name]
            if SdkSwiftOverlayInfo not in sdk_dep:
                continue
            overlay_info = sdk_dep[SdkSwiftOverlayInfo]
            for underlying_module, overlay_modules in overlay_info.overlays.items():
                # Only add a cross import SDK overlay if both modules associated with the overlay are required
                if underlying_module in reachable:
                    # Cross import overlays themselves are always Swift modules, but the underlying module
                    # can be a Swift module or a Clang module
                    for overlay_name in overlay_modules:
                        if overlay_name in swift_map and overlay_name not in seen_overlays:
                            seen_overlays.add(overlay_name)
                            sdk_overlays.append(swift_map[overlay_name])

    return sdk_deps + sdk_overlays
