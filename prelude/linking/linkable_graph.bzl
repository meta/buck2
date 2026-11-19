# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:compile_types.bzl", "CxxSrcCompileCommand")
load("@prelude//cxx:cxx_toolchain_types.bzl", "PicBehavior")
load("@prelude//cxx:headers.bzl", "CPrecompiledHeaderInfo")
# TODO(mattpayne): Add this back once the type is supported by dependency mgmt
# load("@prelude//cxx:shared_library_interface.bzl", "SharedInterfaceInfo")
load("@prelude//linking:types.bzl", "Linkage")
load("@prelude//python:python.bzl", "PythonLibraryInfo")
load("@prelude//utils:expect.bzl", "expect")
load(
    "@prelude//utils:graph_utils.bzl",
    "depth_first_traversal_by",
)
load(
    ":link_info.bzl",
    "LibOutputStyle",
    "LinkInfo",  # @unused Used as a type
    "LinkInfos",
    "LinkStrategy",
    "LinkerFlags",
    "MergedLinkInfo",
    "get_lib_output_style",
    "get_output_styles_for_linkage",
    _get_link_info = "get_link_info",
)
load(
    ":shared_libraries.bzl",
    "SharedLibraries",
)

# A provider with information used to link a rule into a shared library.
# Potential omnibus roots must provide this so that omnibus can link them
# here, in the context of the top-level packaging rule.
LinkableRootInfo = provider(
    # @unsorted-dict-items
    fields = {
        "label": provider_field(Label),
        "link_infos": provider_field(typing.Any, default = None),  # LinkInfos
        "name": provider_field(typing.Any, default = None),  # [str, None]
        "deps": provider_field(typing.Any, default = None),  # ["label"]
    },
)

###############################################################################
# Linkable Graph collects information on a node in the target graph that
# contains linkable output. This graph information may then be provided to any
# consumers of this target.
###############################################################################

_DisallowConstruction = record()
_TargetSourceType = Artifact | str | tuple

LinkableNode = record(
    # Attribute labels on the target.
    labels = field(list[str], []),
    # Preferred linkage for this target.
    preferred_linkage = field(Linkage, Linkage("any")),
    default_link_strategy = field(LinkStrategy),
    # Linkable deps of this target.
    deps = field(list[Label], []),
    # Exported linkable deps of this target.
    #
    # We distinguish between deps and exported deps so that when creating shared
    # libraries in a large graph we only need to link each library against its
    # deps and their (transitive) exported deps. This helps keep link lines smaller
    # and produces more efficient libs (for example, DT_NEEDED stays a manageable size).
    exported_deps = field(list[Label], []),
    # List of both deps and exported deps. We traverse linkable graph lots of times
    # and preallocating this list saves RAM during analysis
    all_deps = field(list[Label], []),
    # Link infos for all supported lib output styles supported by this node. This should have a value
    # for every output_style supported by the preferred linkage.
    link_infos = field(dict[LibOutputStyle, LinkInfos], {}),
    # Contains the linker flags for this node.
    # Note: The values in link_infos will already be adding in the exported_linker_flags
    # TODO(cjhopman): We should probably make all use of linker_flags explicit, but that may need to wait
    # for all link strategies to operate on the LinkableGraph.
    linker_flags = field(LinkerFlags),
    # Shared libraries provided by this target.  Used if this target is
    # excluded.
    shared_libs = field(SharedLibraries, SharedLibraries(libraries = [])),
    # The soname this node would use in default link strategies. May be used by non-default
    # link strategies as a lib's soname.
    default_soname = field(str | None),
    # Records Android's can_be_asset value for the node. This indicates whether the node can be bundled
    # as an asset in android apks.
    can_be_asset = field(bool),
    # Collected target sources from the target.
    srcs = field(list[_TargetSourceType]),
    # Whether the node should appear in the android mergemap (which provides information about the original
    # soname->final merged lib mapping)
    include_in_android_mergemap = field(bool),
    # Don't follow dependents on this node even if has preferred linkage static
    ignore_force_static_follows_dependents = field(bool),
    # Should this library only be used for build time linkage
    stub = field(bool),
    supports_shlib_interfaces = field(bool, True),
    # Per-source C/C++ compile commands for this node, used to aggregate an app-level native
    # compile-command database (the [native_build_commands][compile] sub-target). Populated only when
    # compile-command capture is enabled (see cxx_library); empty otherwise and for nodes with no
    # sources (e.g. prebuilt libraries).
    compile_cmds = field(list[CxxSrcCompileCommand], []),
    # Only allow constructing within this file.
    _private = _DisallowConstruction,
)

LinkableGraphNode = record(
    # Target/label of this node
    label = field(Label),
    # If this node has linkable output, it's linkable data
    linkable = field([LinkableNode, None]),
    # All potential root notes for an omnibus link (e.g. C++ libraries,
    # C++ Python extensions).
    roots = field(dict[Label, LinkableRootInfo]),
    # Exclusions this node adds to the Omnibus graph
    excluded = field(dict[Label, None]),
    # Only allow constructing within this file.
    _private = _DisallowConstruction,
)

LinkableGraphTSet = transitive_set()

# The LinkableGraph for a target holds all the transitive nodes, roots, and exclusions
# from all of its dependencies.
#
# TODO(cjhopman): Rather than flattening this at each node, we should build up an actual
# graph structure.
LinkableGraph = provider(
    fields = {
        # Target identifier of the graph.
        "label": provider_field(typing.Any, default = None),  # Label
        "nodes": provider_field(typing.Any, default = None),  # "LinkableGraphTSet"
    }
)

# Used to tag a rule as providing a shared native library that may be loaded
# dynamically, at runtime (e.g. via `dlopen`).
DlopenableLibraryInfo = provider(
    fields = {
        # The library ships its own prebuilt shared library, which should be used
        # as-is rather than re-linked from its archive. The prebuilt one may have
        # properties that a re-link would lose, e.g. symbol versioning that
        # external consumers depend on.
        "prebuilt_shared_lib": provider_field(bool, default = False),
    }
)

DLOPENABLE_LIBRARY_INFO_MARKER = DlopenableLibraryInfo()

def _get_required_outputs_for_linkage(linkage: Linkage) -> list[LibOutputStyle]:
    if linkage == Linkage("shared"):
        return [LibOutputStyle("shared_lib")]

    return get_output_styles_for_linkage(linkage)

def _get_target_sources(ctx: AnalysisContext) -> list[_TargetSourceType]:
    srcs = []
    if hasattr(ctx.attrs, "srcs"):
        srcs.extend(ctx.attrs.srcs)
    return srcs

def create_linkable_node(
    ctx: AnalysisContext,
    default_soname: str | None,
    preferred_linkage: Linkage = Linkage("any"),
    default_link_strategy: LinkStrategy = LinkStrategy("shared"),
    deps: list[Dependency | LinkableGraph] = [],
    exported_deps: list[Dependency | LinkableGraph] = [],
    link_infos: dict[LibOutputStyle, LinkInfos] = {},
    shared_libs: SharedLibraries = SharedLibraries(libraries = []),
    can_be_asset: bool = True,
    include_in_android_mergemap: bool = True,
    linker_flags: [LinkerFlags, None] = None,
    ignore_force_static_follows_dependents: bool = False,
    stub: bool = False,
    compile_cmds: list[CxxSrcCompileCommand] = [],
) -> LinkableNode:
    for output_style in _get_required_outputs_for_linkage(preferred_linkage):
        expect(
            output_style in link_infos,
            "must have {} link info".format(output_style),
        )
    if not linker_flags:
        linker_flags = LinkerFlags()
    deps = linkable_deps(deps)
    exported_deps = linkable_deps(exported_deps)
    return LinkableNode(
        labels = ctx.attrs.labels,
        preferred_linkage = preferred_linkage,
        default_link_strategy = default_link_strategy,
        deps = deps,
        exported_deps = exported_deps,
        all_deps = deps + exported_deps,
        link_infos = link_infos,
        shared_libs = shared_libs,
        can_be_asset = can_be_asset,
        srcs = _get_target_sources(ctx),
        include_in_android_mergemap = include_in_android_mergemap,
        default_soname = default_soname,
        linker_flags = linker_flags,
        ignore_force_static_follows_dependents = ignore_force_static_follows_dependents,
        stub = stub,
        supports_shlib_interfaces = getattr(ctx.attrs, "supports_shlib_interfaces", True),
        compile_cmds = compile_cmds,
        _private = _DisallowConstruction(),
    )

def create_linkable_graph_node(
    ctx: AnalysisContext,
    linkable_node: [LinkableNode, None] = None,
    roots: dict[Label, LinkableRootInfo] = {},
    excluded: dict[Label, None] = {},
    label: Label | None = None,
) -> LinkableGraphNode:
    if not label:
        label = ctx.label
    return LinkableGraphNode(
        label = label,
        linkable = linkable_node,
        roots = roots,
        excluded = excluded,
        _private = _DisallowConstruction(),
    )

def create_linkable_graph(
    ctx: AnalysisContext,
    node: [LinkableGraphNode, None] = None,
    # This list of deps must include all deps referenced by the LinkableGraphNode.
    deps: list[[LinkableGraph, Dependency]] = [],
) -> LinkableGraph:
    graph_deps = []
    for d in deps:
        if isinstance(d, LinkableGraph):
            graph_deps.append(d)
        else:
            graph = d.get(LinkableGraph)
            if graph:
                graph_deps.append(graph)

    if node and node.linkable:
        deps_labels = set([x.label for x in graph_deps])
        for l in [node.linkable.deps, node.linkable.exported_deps]:  # buildifier: disable=confusing-name
            for d in l:
                if not d in deps_labels:
                    fail(
                        "LinkableNode had {} in its deps, but that label is missing from the node's linkable graph children (`{}`)".format(
                            d, ", ".join(deps_labels)
                        )
                    )

    children = [x.nodes for x in graph_deps]

    kwargs = {
        "children": children,
    }
    if node:
        kwargs["value"] = node
        label = node.label
    else:
        label = ctx.label
    return LinkableGraph(
        label = label,
        nodes = ctx.actions.tset(LinkableGraphTSet, **kwargs),
    )

ReducedLinkableGraph = record(
    # Label to information map for whole graph.
    # Does not have entry for executable
    nodes = field(dict[Label, LinkableNode]),
    # Order of linkable in the graph as it would go into linker argsfile
    # when building executable in static link strategy
    link_order = field(dict[Label, int]),
)

def reduce_linkable_graph(graph: LinkableGraph) -> ReducedLinkableGraph:
    linkable_nodes = {}
    link_order = {}

    # Link groups machinery may be used by something that does not
    # store all information in dependency graph. E.g. python native dlopen
    # So gathering link ordering starting from executable label may not collect all
    # dependencies correctly. To account for that we add remaining pieces to
    # final result. There is no particular reasoning behing putting remaining linkables first,
    # but it is just more convenient to implement.
    # So to make it work properly we start with `1` instead of `0` and return default `0` for linkables
    # that we did not put into linkable graph nodes.
    link_order_idx = 1

    for node in filter(None, graph.nodes.traverse()):
        if node.linkable:
            linkable_nodes[node.label] = node.linkable
            link_order[node.label] = link_order_idx
            link_order_idx += 1

    return ReducedLinkableGraph(
        nodes = linkable_nodes,
        link_order = link_order,
    )

def get_linkable_graph_node_map_func(graph: LinkableGraph):
    def get_linkable_graph_node_map() -> dict[Label, LinkableNode]:
        nodes = graph.nodes.traverse()
        linkable_nodes = {}
        for node in filter(None, nodes):
            if node.linkable:
                linkable_nodes[node.label] = node.linkable
        return linkable_nodes

    return get_linkable_graph_node_map

def linkable_deps(deps: list[Dependency | LinkableGraph]) -> list[Label]:
    labels = []

    for dep in deps:
        if isinstance(dep, LinkableGraph):
            labels.append(dep.label)
        else:
            dep_info = linkable_graph(dep)
            if dep_info != None:
                labels.append(dep_info.label)

    return labels

def linkable_graph(dep: Dependency) -> [LinkableGraph, None]:
    """
    Helper to extract `LinkableGraph` from a dependency which also
    provides `MergedLinkInfo`.
    """

    # We only care about "linkable" deps.
    if PythonLibraryInfo in dep or MergedLinkInfo not in dep or dep.label.sub_target == ["headers"]:
        return None

    if CPrecompiledHeaderInfo in dep:
        # `cxx_precompiled_header()` does not contribute to the link, only to compile
        return None

    return dep[LinkableGraph]

def get_link_info(node: LinkableNode, output_style: LibOutputStyle, prefer_stripped: bool = False, prefer_optimized: bool = False) -> LinkInfo:
    info = _get_link_info(
        node.link_infos[output_style],
        prefer_stripped = prefer_stripped,
        prefer_optimized = prefer_optimized,
    )
    return info

def get_deps_for_link(
    node: LinkableNode, strategy: LinkStrategy, pic_behavior: PicBehavior, overridden_preferred_linkage: Linkage | None = None
) -> list[Label]:
    """
    Return deps to follow when linking against this node with the given link
    style.
    """

    # If we're linking statically, include non-exported deps.
    output_style = get_lib_output_style(strategy, overridden_preferred_linkage if overridden_preferred_linkage else node.preferred_linkage, pic_behavior)
    if output_style != LibOutputStyle("shared_lib"):
        return node.all_deps
    else:
        return node.exported_deps

def get_transitive_deps(link_infos: dict[Label, LinkableNode], roots: list[Label]) -> list[Label]:
    """
    Return all transitive deps from following the given nodes.
    """

    def find_transitive_deps(node: Label):
        return link_infos[node].all_deps

    return depth_first_traversal_by(link_infos, roots, find_transitive_deps)
