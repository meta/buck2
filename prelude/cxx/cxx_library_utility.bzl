# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifacts.bzl",
    "ArtifactOutputs",  # @unused Used as a type
    "single_artifact",
)
load("@prelude//:paths.bzl", "paths")
load(
    "@prelude//linking:link_info.bzl",
    "DepMetadata",
    "LinkStyle",
    "LinkerFlags",
    "MergedLinkInfo",
)
load("@prelude//linking:types.bzl", "Linkage")
load(
    "@prelude//utils:utils.bzl",
    "filter_and_map_idx",
    "from_named_set",
)
load(":cxx_context.bzl", "get_cxx_platform_info", "get_cxx_toolchain_info")
load(":cxx_flags.bzl", "cxx_flags_linker_flags")
load(
    ":cxx_toolchain_types.bzl",
    "LinkerType",
    "ShlibInterfacesMode",
)

OBJECTS_SUBTARGET = "objects"

EMPTY_DEFAULT_INFO = DefaultInfo()

# The dependencies, and the default deps (if selected)
def cxx_attr_deps(ctx: AnalysisContext) -> list[Dependency]:
    deps = ctx.attrs.deps

    if getattr(ctx.attrs, "default_deps", "") == "deps":
        toolchain_info = get_cxx_toolchain_info(ctx)
        if toolchain_info.default_deps:
            deps.extend(toolchain_info.default_deps)

    deps_query_attr = getattr(ctx.attrs, "deps_query", None)
    if deps_query_attr:
        return deps + deps_query_attr

    # Avoid making a copy of deps if deps_query is not set.
    return deps

# The exported dependencies, and the default deps (if selected)
def cxx_attr_exported_deps(ctx: AnalysisContext) -> list[Dependency]:
    exported_deps = []

    exported_deps_attr = getattr(ctx.attrs, "exported_deps", None)
    if exported_deps_attr:
        exported_deps.extend(exported_deps_attr)

    if getattr(ctx.attrs, "default_deps", "") == "exported_deps":
        toolchain_info = get_cxx_toolchain_info(ctx)
        if toolchain_info.default_deps:
            exported_deps.extend(toolchain_info.default_deps)

    return exported_deps

def cxx_attr_linker_flags_all(ctx: AnalysisContext, cxx_flags: list[Dependency] = []) -> LinkerFlags:
    flags = cxx_attr_linker_flags(ctx, cxx_flags)

    local_linker_script_flags_attr = getattr(ctx.attrs, "local_linker_script_flags", None)
    if local_linker_script_flags_attr:
        flags.extend(local_linker_script_flags_attr)

    post_flags = getattr(ctx.attrs, "post_linker_flags", [])

    return LinkerFlags(
        flags = flags,
        post_flags = post_flags,
        exported_flags = ctx.attrs.exported_linker_flags,
        exported_post_flags = ctx.attrs.exported_post_linker_flags,
    )

def cxx_inherited_link_info(first_order_deps: list[Dependency]) -> list[MergedLinkInfo]:
    """
    Returns the list of MergedLinkInfo from the dependencies, filtering out those without a MergedLinkInfo
    """

    # We filter out nones because some non-cxx rule without such providers could be a dependency, for example
    # cxx_binary "fbcode//one_world/cli/util/process_wrapper:process_wrapper" depends on
    # python_library "fbcode//third-party-buck/$platform/build/glibc:__project__"
    return filter_and_map_idx(MergedLinkInfo, first_order_deps)

# Linker flags
def cxx_attr_linker_flags(ctx: AnalysisContext, cxx_flags: list[Dependency] = []) -> list[typing.Any]:
    linker_flags = cxx_flags_linker_flags(ctx.actions, cxx_flags) + ctx.attrs.linker_flags
    return linker_flags

def cxx_attr_link_style(ctx: AnalysisContext) -> LinkStyle:
    if ctx.attrs.link_style != None:
        return LinkStyle(ctx.attrs.link_style)
    if ctx.attrs.defaults != None:
        # v1 equivalent code is in CxxConstructorArg::getDefaultFlavors and ParserWithConfigurableAttributes::applyDefaultFlavors
        # Only values in the map are used by v1 as flavors, copy this behavior and return the first value which is compatible with link style.
        v1_flavors = ctx.attrs.defaults.values()
        for s in [LinkStyle("static"), LinkStyle("static_pic"), LinkStyle("shared")]:
            if s.value in v1_flavors:
                return s
    return get_cxx_toolchain_info(ctx).linker_info.link_style

def cxx_attr_preferred_linkage(ctx: AnalysisContext) -> Linkage:
    preferred_linkage = ctx.attrs.preferred_linkage

    # force_static is deprecated, but it has precedence over preferred_linkage
    if getattr(ctx.attrs, "force_static", False):
        preferred_linkage = "static"

    return Linkage(preferred_linkage)

def cxx_attr_resources(ctx: AnalysisContext) -> dict[str, ArtifactOutputs]:
    """
    Return the resources provided by this rule, as a map of resource name to
    a tuple of the resource artifact and any "other" outputs exposed by it.
    """

    resources = {}

    resources_attr = getattr(ctx.attrs, "resources", None)
    if resources_attr:
        # Resource keys prefix with `header_namespace`. Unset (None) falls
        # back to package; explicit "" means "no prefix" UNLESS `raw_headers`
        # is also set, which signals the xplat macros (`_unified_cxx_library`
        # / `_fbcode_cpp_common_wrapper`) clobbered `header_namespace=""` to
        # suppress #include namespace mangling — fall back to package in that
        # case so consumers (e.g. `kBuckPrefix + "manifest.json"`) keep
        # working. Idempotent: keys already prefixed (by ACME, snapshots) are
        # left alone.
        namespace = ctx.attrs.header_namespace
        if namespace == None:
            namespace = ctx.label.package
        elif namespace == "" and getattr(ctx.attrs, "raw_headers", None):
            namespace = ctx.label.package
        prefix = namespace + "/" if namespace else None

        # Use getattr, as apple rules don't have a `resources` parameter.
        for name, resource in from_named_set(resources_attr).items():
            if not namespace:
                key = name
            elif name == namespace or name.startswith(prefix):
                key = name
            else:
                key = paths.join(namespace, name)
            resources[key] = single_artifact(resource)

    return resources

def cxx_is_gnu(ctx: AnalysisContext) -> bool:
    return get_cxx_toolchain_info(ctx).linker_info.type == LinkerType("gnu")

def cxx_use_shlib_intfs(ctx: AnalysisContext) -> bool:
    """
    Return whether we should use shared library interfaces for linking.
    """

    # Per-rule opt-out.
    if not getattr(ctx.attrs, "supports_shlib_interfaces", True):
        return False

    linker_info = get_cxx_toolchain_info(ctx).linker_info
    return linker_info.shlib_interfaces != ShlibInterfacesMode("disabled")

def cxx_use_shlib_intfs_mode(ctx: AnalysisContext, mode: ShlibInterfacesMode) -> bool:
    """
    Verify we are using a specific shared library interface mode.
    """
    return cxx_use_shlib_intfs(ctx) and get_cxx_toolchain_info(ctx).linker_info.shlib_interfaces == mode

def cxx_platform_supported(ctx: AnalysisContext) -> bool:
    """
    Return whether this rule's `supported_platforms_regex` matches the current
    platform name.
    """

    if ctx.attrs.supported_platforms_regex == None:
        return True

    return regex_match(
        ctx.attrs.supported_platforms_regex,
        get_cxx_platform_info(ctx).name,
    )

def cxx_attr_dep_metadata(ctx: AnalysisContext) -> list[DepMetadata]:
    """
    Return a `DepMetadata` structure with a meaningful version identifier.
    """
    if not getattr(ctx.attrs, "version", None):
        return []
    return [DepMetadata(version = ctx.attrs.version)]

def cxx_attr_use_content_based_paths(ctx: AnalysisContext) -> bool:
    """
    Return whether this rule should use content-based paths.
    """
    return getattr(ctx.attrs, "use_content_based_paths", False)
