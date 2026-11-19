# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//apple/swift:swift_compilation.bzl", "extract_swiftmodule_linkables", "get_swiftmodule_linker_flags")
load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo", "LinkerType")
load(
    "@prelude//cxx:transformation_spec.bzl",
    "TransformationSpecContext",  # @unused Used as a type
)
load(
    "@prelude//linking:link_info.bzl",
    "FrameworksLinkable",
    "LinkArgs",
    "LinkInfo",
    "LinkStrategy",  # @unused Used as a type
    "MergedLinkInfo",
    "SwiftmoduleLinkable",  # @unused Used as a type
    "get_link_args_for_strategy",
    "has_framework_linkable",
    "merge_swiftmodule_linkables",
)
load("@prelude//utils:expect.bzl", "expect")
load(":apple_framework_versions.bzl", "get_framework_linker_args", "validate_sdk_frameworks")
load(":apple_toolchain_types.bzl", "AppleToolchainInfo")
load(":apple_utility.bzl", "has_apple_toolchain")

_IMPLICIT_SDKROOT_FRAMEWORK_SEARCH_PATHS = [
    "$SDKROOT/Library/Frameworks",
    "$SDKROOT/System/Library/Frameworks",
    "$SDKROOT/System/Library/PrivateFrameworks",
]

def apple_create_frameworks_linkable(ctx: AnalysisContext) -> [FrameworksLinkable, None]:
    if not ctx.attrs.libraries and not ctx.attrs.frameworks:
        return None

    return FrameworksLinkable(
        libraries = ctx.attrs.libraries,
        frameworks = ctx.attrs.frameworks,
    )

def _has_darwin_linker(ctx: AnalysisContext) -> bool:
    return has_apple_toolchain(ctx) or ctx.attrs._cxx_toolchain[CxxToolchainInfo].linker_info.type == LinkerType("darwin")

def _get_apple_frameworks_linker_flags(ctx: AnalysisContext, linkables: list[[FrameworksLinkable, None]]) -> cmd_args:
    if not has_framework_linkable(linkables):
        return cmd_args()

    frameworks = set()
    libraries = set()
    for linkable in linkables:
        if not linkable:
            continue

        frameworks.update(linkable.frameworks)
        libraries.update(linkable.libraries)

    framework_names = [to_framework_name(framework) for framework in frameworks]
    unresolved_framework_paths = _get_non_sdk_unresolved_framework_directories(frameworks)

    expanded_frameworks_paths = _expand_sdk_framework_paths(ctx, unresolved_framework_paths)
    flags = _get_framework_search_path_flags(expanded_frameworks_paths)
    flags.add(get_framework_linker_args(ctx, framework_names))
    flags.add(cmd_args([_library_name(library) for library in libraries], format = "-l{}"))

    return flags

def get_framework_search_path_flags(ctx: AnalysisContext) -> cmd_args:
    if not _has_darwin_linker(ctx):
        return cmd_args()

    validate_sdk_frameworks(ctx.attrs.frameworks)
    unresolved_framework_dirs = _get_non_sdk_unresolved_framework_directories(ctx.attrs.frameworks)
    expanded_framework_dirs = _expand_sdk_framework_paths(ctx, unresolved_framework_dirs)
    return _get_framework_search_path_flags(expanded_framework_dirs)

def _get_framework_search_path_flags(frameworks: list[cmd_args]) -> cmd_args:
    flags = cmd_args()
    for directory in frameworks:
        flags.add(cmd_args(directory, format = "-F{}"))

    return flags

def _get_non_sdk_unresolved_framework_directories(frameworks: typing.Iterable) -> set[str]:
    # We don't want to include SDK directories as those are already added via `isysroot` flag in toolchain definition.
    # Adding those directly via `-F` will break building Catalyst applications as frameworks from support directory
    # won't be found and those for macOS platform will be used.
    directories = set()
    for framework in frameworks:
        directory = _non_sdk_unresolved_framework_directory(framework)
        if directory:
            directories.add(directory)

    return directories

def to_framework_name(framework_path: str) -> str:
    name, ext = paths.split_extension(paths.basename(framework_path))
    expect(ext == ".framework", "framework `{}` missing `.framework` suffix", framework_path)
    return name

def _library_name(library: str) -> str:
    if ":" in library:
        fail("Invalid library: {}. Use the field 'linker_flags' with $(location ) macro if you want to pass in a BUCK target for libraries.".format(library))

    name = paths.basename(library)
    if not name.startswith("lib"):
        fail("unexpected library: {}".format(library))
    return paths.split_extension(name[3:])[0]

def _expand_sdk_framework_paths(ctx: AnalysisContext, unresolved_framework_paths: set[str]) -> list[cmd_args]:
    # When _apple_toolchain is not available (e.g., cxx_library used as apple_library swap),
    # we cannot expand $PLATFORM_DIR or $SDKROOT paths. Skip those paths gracefully.
    if not hasattr(ctx.attrs, "_apple_toolchain"):
        return [cmd_args(p) for p in unresolved_framework_paths if not p.startswith("$")]
    return [_expand_sdk_framework_path(ctx, unresolved_framework_path) for unresolved_framework_path in unresolved_framework_paths]

def _expand_sdk_framework_path(ctx: AnalysisContext, framework_path: str) -> cmd_args:
    apple_toolchain_info = ctx.attrs._apple_toolchain[AppleToolchainInfo]
    path_expansion_map = {
        "$PLATFORM_DIR/": apple_toolchain_info.platform_path,
        "$SDKROOT/": apple_toolchain_info.sdk_path,
    }

    for trailing_path_variable, path_value in path_expansion_map.items():
        (before, separator, relative_path) = framework_path.partition(trailing_path_variable)
        if separator == trailing_path_variable:
            if len(before) > 0:
                fail("Framework symbolic path not anchored at the beginning, tried expanding `{}`".format(framework_path))
            if relative_path.count("$") > 0:
                fail("Framework path contains multiple symbolic paths, tried expanding `{}`".format(framework_path))
            if len(relative_path) == 0:
                fail(
                    "Framework symbolic path contains no relative path to expand, tried expanding `{}`, relative path: `{}`, before: `{}`, separator `{}`".format(
                        framework_path, relative_path, before, separator
                    )
                )

            return cmd_args([path_value, relative_path], delimiter = "/")

    if framework_path.find("$") == 0:
        fail("Failed to expand framework path: {}".format(framework_path))

    return cmd_args(framework_path)

def _non_sdk_unresolved_framework_directory(framework_path: str) -> [str, None]:
    # We must only drop any framework paths that are part of the implicit
    # framework search paths in the linker + compiler, all other paths
    # must be expanded and included as part of the command.
    for implicit_search_path in _IMPLICIT_SDKROOT_FRAMEWORK_SEARCH_PATHS:
        if framework_path.find(implicit_search_path) == 0:
            return None
    return paths.dirname(framework_path)

def apple_build_link_args_with_deduped_flags(
    ctx: AnalysisContext,
    deps_merged_link_infos: list[MergedLinkInfo],
    frameworks_linkable: [FrameworksLinkable, None],
    link_strategy: LinkStrategy,
    transformation_spec_context: TransformationSpecContext | None,
    swiftmodule_linkable: [SwiftmoduleLinkable, None] = None,
    prefer_stripped: bool = False,
) -> LinkArgs:
    if _has_darwin_linker(ctx):
        frameworks_linkables = [x.frameworks[link_strategy] for x in deps_merged_link_infos] + [frameworks_linkable]
        swiftmodule_linkables = [x.swiftmodules[link_strategy] for x in deps_merged_link_infos] + [swiftmodule_linkable]
        link_info = _apple_link_info_from_linkables(
            ctx,
            frameworks_linkables,
            swiftmodule_linkables,
        )
    else:
        link_info = None

    return get_link_args_for_strategy(
        ctx.actions,
        ctx.label,
        get_cxx_toolchain_info(ctx).linker_info,
        deps_merged_link_infos,
        link_strategy,
        prefer_stripped,
        transformation_spec_context,
        additional_link_info = link_info,
    )

def apple_get_link_info_by_deduping_link_infos(
    ctx: AnalysisContext,
    infos: list[[LinkInfo, None]],
    framework_linkable: [FrameworksLinkable, None] = None,
    swiftmodule_linkable: [SwiftmoduleLinkable, None] = None,
) -> [LinkInfo, None]:
    if not _has_darwin_linker(ctx):
        return None

    # When building a framework or executable, all frameworks used by the statically-linked
    # deps in the subtree need to be linked.
    #
    # Without deduping, we've seen the linking step fail because the argsfile
    # exceeds the acceptable size by the linker.
    framework_linkables = _extract_framework_linkables(infos)
    if framework_linkable:
        framework_linkables.append(framework_linkable)

    swiftmodule_linkables = extract_swiftmodule_linkables(infos)
    swiftmodule_linkables.append(swiftmodule_linkable)

    return _apple_link_info_from_linkables(ctx, framework_linkables, swiftmodule_linkables)

def _extract_framework_linkables(link_infos: [list[LinkInfo], None]) -> list[FrameworksLinkable]:
    linkables = []
    for merged in link_infos:
        for linkable in merged.linkables:
            if isinstance(linkable, FrameworksLinkable):
                linkables.append(linkable)

    return linkables

def _apple_link_info_from_linkables(
    ctx: AnalysisContext, framework_linkables: list[[FrameworksLinkable, None]], swiftmodule_linkables: list[[SwiftmoduleLinkable, None]] = []
) -> [LinkInfo, None]:
    """
    Returns a LinkInfo for the frameworks, swiftmodules, and swiftruntimes or None if there's none of those.
    """
    framework_link_args = _get_apple_frameworks_linker_flags(ctx, framework_linkables)
    sdk_swift_module_link_args = get_swiftmodule_linker_flags(ctx, merge_swiftmodule_linkables(ctx, swiftmodule_linkables))

    return (
        LinkInfo(
            pre_flags = [framework_link_args, sdk_swift_module_link_args],
        )
        if (framework_link_args or sdk_swift_module_link_args)
        else None
    )
