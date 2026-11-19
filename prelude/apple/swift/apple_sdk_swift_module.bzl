# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//apple:apple_utility.bzl", "get_base_swiftinterface_compilation_flags")
load(":swift_toolchain_types.bzl", "SdkSwiftOverlayInfo", "SdkUncompiledModuleInfo")

def apple_sdk_swift_module_impl(ctx: AnalysisContext) -> list[Provider]:
    module_name = ctx.attrs.module_name
    cmd = get_base_swiftinterface_compilation_flags(module_name)

    if module_name == "Swift" or module_name == "SwiftOnoneSupport":
        cmd.add([
            "-parse-stdlib",
        ])

    overlays = []
    if ctx.attrs.overlays:
        overlays = [SdkSwiftOverlayInfo(overlays = ctx.attrs.overlays)]

    # The target triple varies in swiftinterface files to use either
    # macos or macosx. To avoid unnecessary module compilation we align
    # on macosx.
    target = ctx.attrs.target
    if "macos" in target and "macosx" not in target:
        target = target.replace("macos", "macosx")

    module_info = SdkUncompiledModuleInfo(
        cxx_deps = ctx.attrs.cxx_deps,
        deps = ctx.attrs.deps,
        input_relative_path = ctx.attrs.swiftinterface_relative_path,
        is_framework = ctx.attrs.is_framework,
        is_swiftmodule = True,
        module_name = ctx.attrs.module_name,
        overlays_transitive_deps = ctx.attrs.overlays_transitive_deps,
        partial_cmd = cmd,
        target = target,
    )

    return [
        DefaultInfo(),
        module_info,
    ] + overlays

# This rule represent a Swift module from SDK and forms a graph of dependencies between such modules.
apple_sdk_swift_module = rule(
    impl = apple_sdk_swift_module_impl,
    attrs = {
        "cxx_deps": attrs.list(attrs.dep(), default = []),
        "deps": attrs.list(attrs.dep(), default = []),
        "is_framework": attrs.bool(default = False),
        # This is a real module name, contrary to `name`
        # which has a special suffix to distinguish Swift and Clang modules with the same name
        "module_name": attrs.string(),
        "overlays": attrs.dict(key = attrs.string(), value = attrs.list(attrs.string(), default = []), sorted = False, default = {}),
        # Transitive closure filtered to overlay owners and underlyings.
        "overlays_transitive_deps": attrs.list(attrs.string(), default = []),
        # A prefixed path ($SDKROOT/$PLATFORM_DIR) to swiftinterface textual file.
        "swiftinterface_relative_path": attrs.option(attrs.string(), default = None),  # if `swiftinterface` is None represents a Root node.
        "target": attrs.string(),
    },
)
