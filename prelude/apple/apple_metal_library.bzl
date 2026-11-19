# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")
load("@prelude//cxx:cxx_context.bzl", "get_cxx_platform_info")
load("@prelude//cxx:target_sdk_version.bzl", "get_versioned_metal_target_triple")
load(
    "@prelude//ide_integrations/xcode:data.bzl",
    "XCODE_DATA_SUB_TARGET",
    "XcodeDataInfoKeys",
    "generate_xcode_data",
)
load(":apple_resource_types.bzl", "AppleResourceDestination", "AppleResourceSpec")
load(":apple_target_sdk_version.bzl", "get_min_deployment_version_for_node")
load(":resource_groups.bzl", "create_resource_graph")

def apple_metal_library_impl(ctx: AnalysisContext) -> list[Provider]:
    metallib_archive = _compile_apple_metal_library(ctx)
    resource_spec = AppleResourceSpec(
        files = [metallib_archive],
        destination = AppleResourceDestination("resources"),
    )
    resource_graph = create_resource_graph(
        ctx = ctx,
        labels = ctx.attrs.labels,
        deps = [],
        exported_deps = [],
        resource_spec = resource_spec,
    )

    xcode_data_default_info, xcode_data_info = generate_xcode_data(ctx, "apple_metal_library", None, _xcode_populate_attributes)

    return [
        DefaultInfo(
            sub_targets = {
                XCODE_DATA_SUB_TARGET: xcode_data_default_info,
            },
            default_output = metallib_archive,
        ),
        resource_graph,
        xcode_data_info,
    ]

# Versioning extracted from Metal spec https://developer.apple.com/metal/Metal-Shading-Language-Specification.pdf
_IOS_MIN_OS_TO_METAL_VERSION = {
    "10": "1.2",
    "11": "2.0",
    "12": "2.1",
    "13": "2.2",
    "14": "2.3",
    "15": "2.4",
    "16": "3.0",
    "17": "3.1",
    "18": "3.2",
    "26": "4.0",
    "8": "1.0",
    "9": "1.1",
}

_MACOS_MIN_OS_TO_METAL_VERSION = {
    "10.11": "1.1",
    "10.12": "1.2",
    "10.13": "2.0",
    "10.14": "2.1",
    "10.15": "2.2",
    "11": "2.3",
    "12": "2.4",
    "13": "3.0",
    "14": "3.1",
    "15": "3.2",
    "26": "4.0",
}

def _derive_metal_version_from_min_os_version(ctx: AnalysisContext, min_os_version: str) -> str | None:
    platform_info = get_cxx_platform_info(ctx)
    version_components = min_os_version.split(".")
    if "macosx" in platform_info.name:
        major = version_components[0]
        if major == "10":
            minor = version_components[1] if len(version_components) > 1 else "0"
            key = "{major}.{minor}".format(
                major = major,
                minor = minor,
            )
        else:
            key = major
        return _MACOS_MIN_OS_TO_METAL_VERSION.get(key)
    if "iphone" in platform_info.name or "maccatalyst" in platform_info.name:
        # Mac Catalyst uses iOS versioning numbers
        return _IOS_MIN_OS_TO_METAL_VERSION.get(version_components[0])
    return None

# Reference: `-std=` section in https://developer.apple.com/metal/Metal-Shading-Language-Specification.pdf
def _compute_metal_std_compiler_arg(ctx: AnalysisContext, min_os_version: str) -> list[str]:
    version = ctx.attrs.metal_version or _derive_metal_version_from_min_os_version(ctx, min_os_version)
    if version == None:
        return []

    if "." not in version:
        version = version + ".0"

    is_version_before_unification = version.startswith("1") or version.startswith("2")
    if is_version_before_unification:
        platform_info = get_cxx_platform_info(ctx)

        # Mac Catalyst, i.e., `maccatalyst`, is using iOS args
        is_mac = "macosx" in platform_info.name
        platform_name = "macos" if is_mac else "ios"
        return [
            "-std={platform}-metal{version}".format(
                platform = platform_name,
                version = version,
            )
        ]

    return ["-std=metal{version}".format(version = version)]

def _compile_apple_metal_library(ctx: AnalysisContext) -> Artifact:
    toolchain = ctx.attrs._apple_toolchain[AppleToolchainInfo]
    if toolchain.metal == None or toolchain.metallib == None:
        fail("apple_metal_library can only be built with the metal compiler available in the apple bundle toolchain")

    header_map = {}
    for header_artifact in ctx.attrs.headers:
        import_paths = [
            # Support including via both direct filename and relative-to-target path
            header_artifact.basename,
            header_artifact.short_path,
        ]
        for import_path in import_paths:
            if import_path in header_map:
                fail("Header file duplicate import path conflict: `{}`".format(import_path))
            header_map[import_path] = header_artifact

    header_symlink_tree = None
    if len(header_map):
        header_symlink_tree = ctx.actions.symlinked_dir(str(ctx.label.name) + "_header_symlink_tree", header_map, has_content_based_path = False)

    sysroot = ctx.attrs._apple_toolchain[AppleToolchainInfo].sdk_path
    air_files = []

    min_os_version = get_min_deployment_version_for_node(ctx)
    compile_std_arg = _compute_metal_std_compiler_arg(ctx, min_os_version)
    for metal_file in ctx.attrs.srcs:
        air_output = ctx.actions.declare_output(paths.replace_extension(ctx.attrs.name + "_" + metal_file.basename, ".air"), has_content_based_path = False)
        air_compile_cmd = cmd_args(toolchain.metal)
        air_compile_cmd.add("-target", get_versioned_metal_target_triple(ctx, min_os_version))
        air_compile_cmd.add(compile_std_arg)
        air_compile_cmd.add("-isysroot", sysroot)
        if header_symlink_tree:
            air_compile_cmd.add("-I", header_symlink_tree)
        air_compile_cmd.add("-c", metal_file)
        air_compile_cmd.add(ctx.attrs.metal_compiler_flags)
        air_compile_cmd.add("-o", air_output.as_output())
        ctx.actions.run(air_compile_cmd, category = "apple_metal_air_compile", identifier = "compile_" + ctx.attrs.name + "_" + metal_file.basename)
        air_files.append(air_output)

    if ctx.attrs.out != None:
        if not ctx.attrs.out.endswith(".metallib"):
            fail("Metal library output name must end in `.metallib`")
        output_name = ctx.attrs.out
    else:
        output_name = ctx.attrs.name + ".metallib"
    output = ctx.actions.declare_output(output_name, has_content_based_path = False)
    metallib_compile = cmd_args(toolchain.metallib)
    for air_file in air_files:
        metallib_compile.add(air_file)
    metallib_compile.add(ctx.attrs.metal_linker_flags)
    metallib_compile.add("-o", output.as_output())

    ctx.actions.run(
        metallib_compile,
        category = "apple_metallib_link",
        identifier = "link_metallib_" + ctx.attrs.name,
        # It's possible a local machine has the correct Xcode version selected
        # but does not have the Metal toolchain installed. Because this would
        # lead to a build failure, we have to send actions remotely where we
        # it's guaranteed the Metal toolchain will be installed alongside Xcode.
        prefer_remote = True,
    )

    return output

def _xcode_populate_attributes(ctx) -> dict[str, typing.Any]:
    return {
        XcodeDataInfoKeys.SRCS: ctx.attrs.srcs,
        XcodeDataInfoKeys.HEADERS: ctx.attrs.headers,
    }
