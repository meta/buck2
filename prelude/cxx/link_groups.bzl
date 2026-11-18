# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load(
    "@prelude//cxx:groups_types.bzl",
    "Group",
)
load(
    "@prelude//cxx:link_groups_types.bzl",
    "LinkGroupDefinitions",
    "LinkGroupInfo",
    "LinkGroupsDebugLinkInfo",
    "LinkGroupsDebugLinkableEntry",
    "LinkGroupsDebugLinkableEntryExtras",
    "LinkGroupsDebugLinkableItem",
)
load(
    "@prelude//cxx:transformation_spec.bzl",
    "TransformationSpecContext",
)
load("@prelude//linking:execution_preference.bzl", "LinkExecutionPreference")
load(
    "@prelude//linking:link_groups.bzl",
    "LinkGroupLib",
)
load(
    "@prelude//linking:link_info.bzl",
    "LibOutputStyle",
    "LinkArgs",
    "LinkInfo",
    "LinkInfos",
    "LinkStrategy",
    "LinkedObject",
    "SharedLibLinkable",
    "get_lib_output_style",
    "get_link_info_for_transformation",
    "link_info_outputs",
    "set_link_info_link_whole",
    "wrap_link_info",
    "wrap_with_no_as_needed_shared_libs_flags",
    get_link_info_from_link_infos = "get_link_info",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "LinkableGraph",
    "LinkableNode",  # @unused Used as a type
    "LinkableRootInfo",
    "ReducedLinkableGraph",
    "create_linkable_graph",
    "get_deps_for_link",
    "get_linkable_graph_node_map_func",
    "get_transitive_deps",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraries",
    "SharedLibrary",
    "Soname",
    "create_shlib",
)
load("@prelude//linking:types.bzl", "Linkage")
load("@prelude//utils:arglike.bzl", "ArgLike")
load("@prelude//utils:expect.bzl", "expect")
load(
    "@prelude//utils:graph_utils.bzl",
    "depth_first_traversal_by",
)
load(
    "@prelude//utils:utils.bzl",
    "value_or",
)
load(":cxx_context.bzl", "get_cxx_toolchain_info")
load(
    ":cxx_library_utility.bzl",
    "cxx_is_gnu",
)
load(":cxx_toolchain_types.bzl", "LinkerType", "PicBehavior")
load(
    ":groups.bzl",
    "compute_mappings",
    "parse_groups_definitions",
)
load(
    ":groups_types.bzl",
    "MATCH_ALL_LABEL",
    "MATCH_DIRECT_DEPS_LABEL",
    "NO_MATCH_LABEL",
    "should_discard_group",
)
load(":hip_debug_extract.bzl", "PRE_EXTRACT_SUFFIX", "hip_debug_extract_available")
load(
    ":link.bzl",
    "CxxLinkerMapData",
    "collect_hip_debug_from_links",
    "cxx_link_shared_library",
    "link_args_have_hip_device_debug",
)
load(
    ":link_types.bzl",
    "link_options",
)
load(
    ":linker.bzl",
    "get_ignore_undefined_symbols_flags",
)
load(
    ":shared_library_interface.bzl",
    "shared_library_interface",
)
load(
    ":symbols.bzl",
    "create_dynamic_list_version_script",
    "extract_global_syms",
    "extract_undefined_syms",
    "get_undefined_symbols_args",
)

# TODO(cjhopman): This deserves some high-level docs about the link group linking strategy.

# TODO(cjhopman): I think it would be good to document/explain why link group linking
# strategy also uses one of the default LinkStrategy.

# Returns a list of targets that belong in the current link group. If using
# auto_link_groups, this sub_target will only be available to use with the
# main binary, and it will return a list of all targets that map to a link
# group.
LINK_GROUP_MAP_DATABASE_SUB_TARGET = "link-group-map-database"
LINK_GROUP_MAP_DATABASE_FILENAME = "link_group_map_database.json"

# Returns the linkables of the link in link order (`LinkGroupsDebugLinkInfo`).
LINK_GROUPS_INFO_SUB_TARGET = "link-groups-info"

# Returns a mapping from link group names to its constituent targets.
LINK_GROUP_MAPPINGS_SUB_TARGET = "link-group-mappings"
LINK_GROUP_MAPPINGS_FILENAME_SUFFIX = ".link_group_map.json"

LinkGroupLinkInfo = record(
    link_info = field(LinkInfo),
    output_style = field(LibOutputStyle),
    # Where this link info is originated from.
    # Either target label or link group name
    link_name = field(Label | str),
    # For a link group entry, the target owning the group's library (`LinkGroupLib.label`);
    # None for a target entry and for a link group the link creates itself.
    owner = field(Label | None, None),
)

LinkGroupLibSpec = record(
    # The output name given to the linked shared object.
    name = field(str),
    # Used to differentiate normal native shared libs from e.g. Python native
    # extensions (which are technically shared libs, but don't set a SONAME
    # and aren't managed by `SharedLibraryInfo`s).
    is_shared_lib = field(bool, True),
    # Optional linkable root info that should be used to "guide" the link of
    # this link group.  This is useful for linking e.g. standalone shared libs
    # which may require special private linker flags (like version scripts) to
    # link.
    root = field([LinkableRootInfo, None], None),
    # The link group to link.
    group = field(Group),
    label = field(Label | None, None),
)

_LinkedLinkGroup = record(
    artifact = field(LinkedObject),
    library = field([LinkGroupLib, None], None),
    linker_map_data = field([CxxLinkerMapData, None], None),
)

_LinkedLinkGroups = record(
    libs = field(dict[str, _LinkedLinkGroup]),
    symbol_ldflags = field(list[typing.Any], []),
    libs_debug_info = field(dict[typing.Any, typing.Any]),
    # Mapping from a target to a link group name it was linked into.
    targets_consumed_by_link_groups = field(dict[Label, str]),
)

def get_link_group(ctx: AnalysisContext) -> [str, None]:
    return ctx.attrs.link_group

def build_link_group_info(graph: LinkableGraph, groups: list[Group], min_node_count: [int, None] = None) -> LinkGroupInfo:
    linkable_graph_node_map = get_linkable_graph_node_map_func(graph)()

    # Filter out groups which don't meet the node count requirement.
    filtered_groups = {}
    node_count = max(value_or(min_node_count, 0), len(linkable_graph_node_map))
    for group in groups:
        if group.attrs.enable_if_node_count_exceeds != None and node_count < group.attrs.enable_if_node_count_exceeds:
            continue
        filtered_groups[group.name] = group

    mappings = compute_mappings(
        groups_map = filtered_groups,
        graph_map = linkable_graph_node_map,
    )

    return LinkGroupInfo(
        groups = filtered_groups,
        groups_hash = hash(str(filtered_groups)),
        mappings = mappings,
        # The consumer of this info may not have deps that cover that labels
        # referenced in our roots, so propagate graphs for them.
        # NOTE(agallagher): We do this to maintain existing behavior here
        # but it's not clear if it's actually desirable behavior.
        graph = graph,
    )

# `executable_deps` and `link_strategy` must be provided unless the
# `link_group_map` is a dependency that outputs the `LinkGroupInfo` provider
def get_link_group_info(
    ctx: AnalysisContext, executable_deps: [list[LinkableGraph], None] = None, link_strategy: LinkStrategy | None = None
) -> [LinkGroupInfo, None]:
    """
    Parses the currently analyzed context for any link group definitions
    and returns a list of all link groups with their mappings.
    """
    link_group_map = ctx.attrs.link_group_map

    if not link_group_map:
        return None

    # If specified as a dep that provides the `LinkGroupInfo`, use that.
    if isinstance(link_group_map, Dependency):
        if LinkGroupDefinitions in link_group_map:
            definitions = link_group_map[LinkGroupDefinitions].definitions(ctx.label, link_strategy)
            if definitions == None:
                return None
            return _get_link_group_info_from_definitions(ctx, executable_deps, definitions)
        return link_group_map[LinkGroupInfo]

    # Otherwise build one from our graph.
    return _get_link_group_info_from_linkable_graph(ctx, executable_deps, ctx.attrs.link_group_map)

def _get_link_group_info_from_linkable_graph(ctx: AnalysisContext, executable_deps: list[LinkableGraph], link_group_map: list) -> LinkGroupInfo:
    link_groups = parse_groups_definitions(link_group_map)
    return _get_link_group_info_from_definitions(ctx, executable_deps, link_groups)

def _get_link_group_info_from_definitions(ctx: AnalysisContext, executable_deps: list[LinkableGraph], link_groups: list[Group]) -> LinkGroupInfo:
    linkable_graph = create_linkable_graph(
        ctx,
        deps = (executable_deps + [d[LinkableGraph] for d in getattr(ctx.attrs, "link_group_deps", [])]),
    )
    return build_link_group_info(
        graph = linkable_graph,
        groups = link_groups,
        min_node_count = getattr(ctx.attrs, "link_group_min_binary_node_count", 0),
    )

def get_link_group_preferred_linkage(link_groups: list[Group]) -> dict[Label, Linkage]:
    root_to_linkage = {}
    for group in link_groups:
        for mapping in group.mappings:
            if not mapping.roots:
                continue

            if not mapping.preferred_linkage:
                continue

            for root in mapping.roots:
                # TODO: There might be a bug here - if the same root is listed in
                # two different link_group_map entries, we'll only use the preferred_linkage
                # of the last style passed.
                root_to_linkage[root] = mapping.preferred_linkage

    return root_to_linkage

LinkGroupContext = record(
    link_group_mappings = field([dict[Label, str], None]),
    # TODO(nga): before, `Any` was `_LinkedLinkGroup`,
    #   which was treated as `Any`, because name starts with underscore.
    #   Put proper type here.
    link_group_libs = field(dict[str, typing.Any]),
    link_group_preferred_linkage = field(dict[Label, Linkage]),
    labels_to_links_map = field(dict[Label, LinkGroupLinkInfo]),
    # Mapping from a target to a link group name it was linked into.
    targets_consumed_by_link_groups = field(dict[Label, str]),
)

def is_link_group_shlib(label: Label, ctx: LinkGroupContext):
    # If this maps to a link group which we have a `LinkGroupLibInfo` for,
    # then we'll handle this outside of this function
    if label in ctx.link_group_mappings and ctx.link_group_mappings[label] in ctx.link_group_libs:
        return False

    # buildifier: disable=uninitialized
    if ctx.link_group_preferred_linkage.get(label, Linkage("any")) == Linkage("shared"):
        return True

    # if using link_groups, only materialize the link_group shlibs
    # buildifier: disable=uninitialized
    node_link = ctx.labels_to_links_map.get(label)
    if node_link != None and node_link.output_style == LibOutputStyle("shared_lib"):
        return True

    return False

BuildLinkGroupsContext = record(
    public_nodes = field(set[Label] | None),
    linkable_graph = field(ReducedLinkableGraph),
    link_groups = field(dict[str, Group]),
    link_group_mappings = field(dict[Label, str] | None),
    link_group_preferred_linkage = field(dict[Label, Linkage]),
    link_strategy = field(LinkStrategy),
    pic_behavior = field(PicBehavior),
    link_group_libs = field(dict[str, ([Label, None], LinkInfos)]),
    link_group_roots = field(dict[str, Label] | None, None),  # If none, derived from link_group_libs
    # False only when the mappings are known to contain no MATCH_DIRECT_DEPS targets.
    may_have_match_direct_deps = field(bool, True),
    prefer_stripped = field(bool, False),
    prefer_optimized = field(bool, False),
    transformation_spec_context = field(TransformationSpecContext | None, None),
)

def _transitively_update_shared_linkage(
    link_group: str | None,
    build_context: BuildLinkGroupsContext,
    link_group_roots: dict[Label, str],
):
    # Reuse the context instead of rechecking its entire typed dictionaries
    # every time we process a link group.
    linkable_graph_node_map = build_context.linkable_graph.nodes
    link_strategy = build_context.link_strategy
    link_group_preferred_linkage = build_context.link_group_preferred_linkage
    pic_behavior = build_context.pic_behavior
    link_group_mappings = build_context.link_group_mappings

    # Identify targets whose shared linkage style may be propagated to
    # dependencies. Implicitly created root libraries are skipped.
    shared_lib_roots = []

    for target in link_group_preferred_linkage:
        # This preferred-linkage mapping may not actually be in the link graph.
        node = linkable_graph_node_map.get(target)
        if node == None:
            continue
        prefered_linkage = link_group_preferred_linkage.get(target, node.preferred_linkage)
        if prefered_linkage != Linkage("static"):
            target_link_group = link_group_roots.get(target)
            if target_link_group == None or target_link_group == link_group:
                shared_lib_roots.append(target)

    # buildifier: disable=uninitialized
    def process_dependency(node: Label) -> list[Label] | None:
        if link_group_mappings and link_group_mappings.get(node) == NO_MATCH_LABEL:
            # Do not propagate shared linkage via nodes that are excluded from link groups.
            return None
        linkable_node = linkable_graph_node_map[node]
        if linkable_node.preferred_linkage == Linkage("any"):
            link_group_preferred_linkage[node] = Linkage("shared")
        return get_deps_for_link(linkable_node, link_strategy, pic_behavior)

    depth_first_traversal_by(
        linkable_graph_node_map,
        shared_lib_roots,
        process_dependency,
    )

def _entry_label(key: Label, link_info: LinkGroupLinkInfo) -> TargetLabel | None:
    """
    The target a `[link-groups-info]` entry stands for. A target entry is its
    own target. A link group entry is keyed by whichever mapped target the
    traversal met first (a member as readily as the group's library), so it
    stands for the target owning the group's library instead, and for none when
    the link created the group itself.
    """
    if type(link_info.link_name) == "string":
        return link_info.owner.raw_target() if link_info.owner else None
    return key.raw_target()

def create_debug_linkable_entries(
    labels_to_links_map: dict[Label, LinkGroupLinkInfo],
    root: Label | None,
    with_extras: bool = False,
    root_link_info: LinkInfo | None = None,
    *,
    entry_cache: dict[(Label | str, LibOutputStyle, bool), LinkGroupsDebugLinkableEntry] | None = None,
    prefer_optimized: bool = False,
) -> list[LinkGroupsDebugLinkableEntry]:
    """
    The entries of `[link-groups-info]`. With `with_extras` (xcode data
    enabled) each entry also carries the target it stands for and the artifacts
    the linker is given for it (`root_link_info` is what the root is linked
    with, when there is a root); otherwise `extras` is None.
    """
    if entry_cache == None:
        entry_cache = {}
    entries = []
    if root:
        root_entry = LinkGroupsDebugLinkableEntry(
            name = root,
            output_style = LibOutputStyle("pic_archive"),
            extras = LinkGroupsDebugLinkableEntryExtras(
                label = root.raw_target(),
                outputs = link_info_outputs(root_link_info) if root_link_info else [],
            )
            if with_extras
            else None,
        )
        entries.append(root_entry)

    for key, link_info in labels_to_links_map.items():
        cache_key = (link_info.link_name, link_info.output_style, prefer_optimized)
        entry = entry_cache.get(cache_key)
        if entry == None:
            entry = LinkGroupsDebugLinkableEntry(
                name = link_info.link_name if type(link_info.link_name) == "string" else link_info.link_name.raw_target(),
                output_style = link_info.output_style,
                extras = LinkGroupsDebugLinkableEntryExtras(
                    label = _entry_label(key, link_info),
                    outputs = link_info_outputs(link_info.link_info),
                )
                if with_extras
                else None,
            )
            entry_cache[cache_key] = entry
        entries.append(entry)

    return entries

# This stores final information about link arguments
# that maps to linker.argsfile for link group or final binary.
FinalLabelsToLinks = record(
    # Static archives and shared libraries inputs.
    map = field(dict[Label, LinkGroupLinkInfo]),
)

def _collect_all_linkables(
    linkable_graph: ReducedLinkableGraph,
    is_executable_link: bool,
    link_strategy: LinkStrategy,
    link_group_preferred_linkage: dict[Label, Linkage],
    pic_behavior: PicBehavior,
    link_group_roots: dict[str, set[Label]],
) -> dict[str, list[Label]]:
    """
    Given a dict of direct roots (that should be linked as archive) for each link group collects
    all linkables for link groups that should be linked as archive and dynamically
    """
    linkables = {}

    for link_group, roots in link_group_roots.items():
        linkables[link_group] = collect_linkables(
            linkable_graph,
            is_executable_link,
            link_strategy,
            link_group_preferred_linkage,
            pic_behavior,
            roots,
        )

    return linkables

def collect_linkables(
    linkable_graph: ReducedLinkableGraph,
    is_executable_link: bool,
    link_strategy: LinkStrategy,
    link_group_preferred_linkage: dict[Label, Linkage],
    pic_behavior: PicBehavior,
    roots: set[Label],
) -> list[Label]:
    def get_potential_linkables(node: Label) -> list[Label]:
        linkable_node = linkable_graph.nodes[node]
        if not is_executable_link and node in roots:
            return linkable_node.all_deps
        return get_deps_for_link(
            linkable_node,
            link_strategy,
            pic_behavior,
            link_group_preferred_linkage.get(node, linkable_node.preferred_linkage),
        )

    # Get all potential linkable targets
    potential_linkables = depth_first_traversal_by(
        linkable_graph.nodes,
        roots,
        get_potential_linkables,
    )

    if _should_fixup_link_order(link_strategy):
        # Link groups machinery may be used by something that does not
        # store all information in dependency graph. E.g. python native dlopen
        # So gathering link ordering starting from executable label may not collect all
        # dependencies correctly. To account for that we add remaining pieces to
        # final result. There is no particular reasoning behing putting remaining linkables first,
        # but it is just more convenient to implement.
        # To make it work we start with `1` instead of `0` when we collect link order dict and return default `0` for linkables
        # that we did not put into linkable graph nodes.
        # To my best knowledge at the moment, that is only related to python that does not currently fixes up link ordering
        # and this is more implemented for future-proofing
        potential_linkables = sorted(potential_linkables, key = lambda node: linkable_graph.link_order.get(node, 0))

    return potential_linkables

# TODO(patskovn): We should have proper DFS link order everywhere.
#                 But now certain places fail in `opt` with fixed up link order
#                 so enabling it more gradually with future follow ups.
def _should_fixup_link_order(link_strategy: LinkStrategy) -> bool:
    return link_strategy == LinkStrategy("shared")

def get_filtered_labels_to_links_map(
    link_group: str | None,
    linkables: list[Label],
    is_executable_link: bool,
    build_context: BuildLinkGroupsContext,
    force_static_follows_dependents: bool = True,
    prefer_optimized = False,
) -> FinalLabelsToLinks:
    """
    Given a linkable graph, link style and link group mappings, finds all links
    to consider for linking traversing the graph as necessary and then
    identifies which link infos and targets belong the in the provided link group.
    If no link group is provided, all unmatched link infos are returned.
    """

    # An index of target to link group names, for all link group library nodes.
    # Provides fast lookup of a link group root lib via it's label.
    link_group_roots = build_context.link_group_roots
    if link_group_roots == None:
        link_group_roots = {label: name for name, (label, _) in build_context.link_group_libs.items() if label != None}

    # Transitively update preferred linkage to avoid runtime issues from
    # missing dependencies (e.g. for prebuilt shared libs).
    _transitively_update_shared_linkage(
        link_group,
        build_context,
        link_group_roots,
    )

    linkable_map = {}

    # Keep track of whether we've already added a link group to the link line
    # already.  This avoids use adding the same link group lib multiple times,
    # for each of the possible multiple nodes that maps to it.
    link_group_added = set()
    group_srcs = {}

    def add_link(target: Label, output_style: LibOutputStyle):
        infos = build_context.linkable_graph.nodes[target].link_infos[output_style]

        if build_context.transformation_spec_context:
            link_info = get_link_info_for_transformation(
                build_context.transformation_spec_context,
                infos,
                target,
                build_context.prefer_stripped,
            )
        else:
            link_info = get_link_info_from_link_infos(
                infos,
                build_context.prefer_stripped,
                prefer_optimized and output_style != LibOutputStyle("shared_lib"),  # prefer_optimized is only relevant for static libs
            )

        linkable_map[target] = LinkGroupLinkInfo(
            link_info = link_info,
            output_style = output_style,
            link_name = target,
        )

    def add_link_group(target: Label, target_group: str):
        link_group_spec = build_context.link_groups.get(target_group, None)
        if link_group_spec and link_group_spec.attrs.prohibit_file_duplicates and build_context.public_nodes and target in build_context.public_nodes:
            if target_group not in group_srcs:
                group_srcs[target_group] = {}
            target_group_srcs = group_srcs[target_group]
            for src in build_context.linkable_graph.nodes[target].srcs:
                if not isinstance(src, Artifact):
                    # "src" is either source file or source file with list of compilation flags.
                    # We do not handle the case where we have compilation flags attached to source files
                    # because it we don't know is link gonna fail or not. So we let user deal with linker errors if there are any.
                    continue

                previous_target = target_group_srcs.get(src, None)
                if previous_target and previous_target != target:
                    fail(
                        "'{}' artifact included multiple times into '{}' link group. From '{}:{}' and '{}:{}'".format(
                            src, target_group, target.package, target.name, previous_target.package, previous_target.name
                        )
                    )
                else:
                    target_group_srcs[src] = target

        # If we've already added this link group to the link line, we're done.
        if target_group in link_group_added:
            return

        # In some flows, we may not have access to the actual link group lib
        # in our dep tree (e.g. https://fburl.com/code/pddmkptb), so just bail
        # in this case.
        # NOTE(agallagher): This case seems broken, as we're not going to set
        # DT_NEEDED tag correctly, or detect missing syms at link time.
        link_group_lib = build_context.link_group_libs.get(target_group)
        if link_group_lib == None:
            return
        owner, shared_link_infos = link_group_lib

        expect(target_group != link_group)
        link_group_added.add(target_group)
        linkable_map[target] = LinkGroupLinkInfo(
            link_info = get_link_info_from_link_infos(shared_link_infos),
            output_style = LibOutputStyle("shared_lib"),
            link_name = target_group,
            owner = owner,
        )

    filtered_groups = [None, NO_MATCH_LABEL, MATCH_ALL_LABEL, MATCH_DIRECT_DEPS_LABEL]

    # Pre-compute which MATCH_DIRECT_DEPS targets should be included in this link group.
    # A MATCH_DIRECT_DEPS target is included only if there's a target in the current
    # link group which has a direct dependency on that MATCH_DIRECT_DEPS target.
    match_direct_deps_for_current_group = set()
    if build_context.may_have_match_direct_deps:
        for target in linkables:
            target_link_group = build_context.link_group_mappings.get(target)
            if target_link_group == link_group or target_link_group == MATCH_ALL_LABEL:
                node = build_context.linkable_graph.nodes[target]
                for dep in node.deps + node.exported_deps:
                    dep_link_group = build_context.link_group_mappings.get(dep)
                    if dep_link_group == MATCH_DIRECT_DEPS_LABEL:
                        match_direct_deps_for_current_group.add(dep)

    output_style = (
        get_lib_output_style(build_context.link_strategy, Linkage("any"), build_context.pic_behavior)
        if build_context.link_strategy != LinkStrategy("shared")
        else LibOutputStyle("shared_lib")
    )

    for target in linkables:
        node = build_context.linkable_graph.nodes[target]
        target_link_group = build_context.link_group_mappings.get(target)

        link_group_preferred_linkage = build_context.link_group_preferred_linkage.get(target, node.preferred_linkage)

        # We should always add force-static libs to the link.
        is_force_static_lib = (
            force_static_follows_dependents and node.preferred_linkage == Linkage("static") and not node.ignore_force_static_follows_dependents
        )

        # If this belongs to the match all link group, the group currently being
        # evaluated, or is a MATCH_DIRECT_DEPS target that has a direct rdep in this group
        matches_current_link_group = target_link_group == link_group or target_link_group == MATCH_ALL_LABEL or target in match_direct_deps_for_current_group

        if link_group_preferred_linkage == Linkage("shared"):
            # filter out any dependencies to be discarded
            if should_discard_group(build_context.link_groups.get(target_link_group)):
                continue

            # If this target is a link group root library, we
            # 1) don't propagate shared linkage down the tree, and
            # 2) use the provided link info in lieu of what's in the grph.
            root_link_group = link_group_roots.get(target)
            if root_link_group != None and root_link_group != link_group:
                add_link_group(target, root_link_group)
            else:
                add_link(target, LibOutputStyle("shared_lib"))

        else:
            # Shared vs static linkage branches are similar, but separated for
            # clarity and ease of debugging.
            if build_context.link_strategy == LinkStrategy("shared"):
                if (target_link_group and matches_current_link_group) or is_force_static_lib:
                    # Target linked statically if:
                    # 1. It belongs to current link group (unique symbols across graph)
                    # 2. It matches all link groups (can duplicate symbols across graph)
                    # 3. It forces static linkage (can duplicate symbols across graph)
                    if not should_discard_group(build_context.link_groups.get(target_link_group)):
                        add_link(target, LibOutputStyle("pic_archive"))

                elif not target_link_group or target_link_group == NO_MATCH_LABEL:
                    # Target directly linked dynamically if:
                    # 1. It doesn't belong to any link group
                    # 2. It belongs to NO_MATCH group
                    add_link(target, output_style)

                elif target_link_group not in filtered_groups:
                    # Targets linked through other link group dynamically if:
                    # 1. It matches other link group
                    add_link_group(target, target_link_group)

            else:  # static or static_pic
                # Always add force-static libs to the link.
                if is_force_static_lib:
                    if not should_discard_group(build_context.link_groups.get(target_link_group)):
                        add_link(target, output_style)
                elif not target_link_group and not link_group:
                    # Ungrouped linkable targets belong to the unlabeled executable
                    add_link(target, output_style)
                elif is_executable_link and target_link_group == NO_MATCH_LABEL:
                    # Targets labeled NO_MATCH belong to the unlabeled executable
                    add_link(target, output_style)
                elif matches_current_link_group:
                    # If this belongs to the match all link group or the group currently being evaluated
                    add_link(target, output_style)
                elif target_link_group not in filtered_groups:
                    add_link_group(target, target_link_group)

    return FinalLabelsToLinks(
        map = linkable_map,
    )

# Find all link group libraries that are first order deps or exported deps of
# the exectuble or another link group's libs
def get_public_link_group_nodes(
    linkable_graph_node_map: dict[Label, LinkableNode],
    link_group_mappings: [dict[Label, str], None],
    executable_deps: list[Label],
    root_link_group: [str, None],
) -> set[Label]:
    external_link_group_nodes = set()

    # TODO(@christylee): do we need to traverse root link group and NO_MATCH_LABEL exported deps?
    # buildifier: disable=uninitialized
    def crosses_link_group_boundary(current_group: [str, None], new_group: [str, None]):
        # belongs to root binary
        if new_group == root_link_group:
            return False

        if new_group == NO_MATCH_LABEL:
            # Using NO_MATCH with an explicitly defined root_link_group is undefined behavior
            expect(root_link_group == None or root_link_group == NO_MATCH_LABEL)
            return False

        # private node in link group
        if new_group == current_group:
            return False
        return True

    # Check the direct deps of the executable since the executable is not in linkable_graph_node_map
    for label in executable_deps:
        group = link_group_mappings.get(label)
        if crosses_link_group_boundary(root_link_group, group):
            external_link_group_nodes.add(label)

    # identify all nodes with a dependent across group boundaries
    for label, node in linkable_graph_node_map.items():
        current_group = link_group_mappings.get(label)

        for dep in node.deps + node.exported_deps:
            new_group = link_group_mappings.get(dep)
            if crosses_link_group_boundary(current_group, new_group):
                external_link_group_nodes.add(dep)

    SPECIAL_LINK_GROUPS = [MATCH_ALL_LABEL, MATCH_DIRECT_DEPS_LABEL, NO_MATCH_LABEL]

    # Additionally identify exported_deps of those marked nodes (except those included in all groups or included in the main executable).

    # TODO(@christylee): dlopen-able libs that depend on the main executable does not have a
    # linkable internal edge to the main executable. Symbols that are not referenced during the
    # executable link might be dropped unless the dlopen-able libs are linked against the main
    # executable. We need to force export those symbols to avoid undefined symbls.

    # buildifier: disable=uninitialized
    def discover_link_group_linkables(node: Label) -> list[Label]:
        exported_deps = []
        for exported_dep in linkable_graph_node_map[node].exported_deps:
            group = link_group_mappings.get(exported_dep)
            if group != root_link_group and group not in SPECIAL_LINK_GROUPS:
                exported_deps.append(exported_dep)
        return exported_deps

    external_link_group_nodes.update(
        # get transitive exported deps
        depth_first_traversal_by(
            linkable_graph_node_map,
            external_link_group_nodes,
            discover_link_group_linkables,
        ),
    )

    return external_link_group_nodes

def get_filtered_links(labels_to_links_map: dict[Label, LinkGroupLinkInfo], public_link_group_nodes: [set[Label], None] = None) -> list[LinkInfo]:
    if public_link_group_nodes == None:
        return [link_group_info.link_info for link_group_info in labels_to_links_map.values()]
    infos = []
    for label, link_group_info in labels_to_links_map.items():
        info = link_group_info.link_info
        if label in public_link_group_nodes:
            infos.append(set_link_info_link_whole(info))
        else:
            infos.append(info)
    return infos

def get_filtered_targets(labels_to_links_map: dict[Label, LinkGroupLinkInfo]) -> set[TargetLabel]:
    # labels_to_links_map will include entries for shared link group libraries
    # as well as libraries being statically linked into this link unit.
    statically_linked_targets = set()
    for label, info in labels_to_links_map.items():
        if info.output_style != LibOutputStyle("shared_lib"):
            statically_linked_targets.add(label.raw_target())

    return statically_linked_targets

def get_link_group_map_json(ctx: AnalysisContext, targets: set[TargetLabel]) -> DefaultInfo:
    json_map = ctx.actions.write_json(LINK_GROUP_MAP_DATABASE_FILENAME, sorted(targets), pretty = True, has_content_based_path = False)
    return DefaultInfo(default_output = json_map)

def get_link_groups_info_json(ctx: AnalysisContext, labels_to_links_map: dict[Label, LinkGroupLinkInfo], with_extras: bool) -> DefaultInfo:
    """
    The `[link-groups-info]` of a link unit that creates no link groups of its
    own (a shared library linked with link groups): the record the executable
    writes, every linkable in link order under `binary`, with its extras when
    `with_extras`, and no `libs`.
    """
    info = LinkGroupsDebugLinkInfo(
        binary = LinkGroupsDebugLinkableItem(
            ordered_linkables = create_debug_linkable_entries(labels_to_links_map, root = None, with_extras = with_extras),
        ),
        libs = {},
    )
    return DefaultInfo(
        default_output = ctx.actions.write_json(
            ctx.label.name + "." + LINK_GROUPS_INFO_SUB_TARGET + ".json",
            info,
            has_content_based_path = False,
        )
    )

def _find_all_relevant_roots(
    specs: list[LinkGroupLibSpec],
    link_group_mappings: dict[Label, str],  # target label to link group name
    roots: list[Label],
    link_strategy: LinkStrategy,
    linkable_graph: ReducedLinkableGraph,
) -> dict[str, set[Label]]:
    linkable_graph_node_map = linkable_graph.nodes
    relevant_roots = {}
    link_groups_for_full_traversal = set()  # list[str]

    # For shared linkage we need to always traverse whole graph to discover
    # roots because `_collect_linkables` (next step in algorithm) stops linkables discovery
    # early due to shared link strategy semantics
    always_traverse_all_roots = link_strategy == LinkStrategy("shared")

    for spec in specs:
        if spec.root != None:
            relevant_roots[spec.group.name] = set(spec.root.deps)
        else:
            roots_from_mappings, has_empty_root = _get_roots_from_mappings(spec, linkable_graph)
            relevant_roots[spec.group.name] = set(roots_from_mappings)
            if has_empty_root or always_traverse_all_roots:
                link_groups_for_full_traversal.add(spec.group.name)

    def collect_and_traverse_roots(node_target: Label) -> list[Label]:
        node = linkable_graph_node_map.get(node_target)
        if node.preferred_linkage == Linkage("static") and not node.ignore_force_static_follows_dependents:
            return node.all_deps

        node_link_group = link_group_mappings.get(node_target)

        if node_link_group == MATCH_ALL_LABEL:
            # Add node into the list of roots for all link groups
            for link_group in relevant_roots.keys():
                relevant_roots[link_group].add(node_target)
        elif node_link_group in link_groups_for_full_traversal and node_link_group != NO_MATCH_LABEL:
            relevant_roots[node_link_group].add(node_target)

        # For MATCH_DIRECT_DEPS support: if this node belongs to a real link group and has
        # a direct dep on a MATCH_DIRECT_DEPS target, add that target to this link group
        if node_link_group and node_link_group in relevant_roots:
            for dep in node.deps + node.exported_deps:
                dep_link_group = link_group_mappings.get(dep)
                if dep_link_group == MATCH_DIRECT_DEPS_LABEL:
                    relevant_roots[node_link_group].add(dep)

        return node.all_deps

    depth_first_traversal_by(
        linkable_graph_node_map,
        roots,
        collect_and_traverse_roots,
    )

    return relevant_roots

def find_relevant_roots(
    link_group: [str, None] = None, linkable_graph_node_map: dict[Label, LinkableNode] = {}, link_group_mappings: dict[Label, str] = {}, roots: list[Label] = []
):
    # Walk through roots looking for the first node which maps to the current
    # link group.

    def collect_and_traverse_roots(roots, node_target: Label) -> list[Label] | None:
        node = linkable_graph_node_map.get(node_target)
        if node.preferred_linkage == Linkage("static") and not node.ignore_force_static_follows_dependents:
            return node.all_deps

        node_link_group = link_group_mappings.get(node_target)

        if node_link_group == MATCH_ALL_LABEL:
            roots.append(node_target)
        elif node_link_group == link_group:
            roots.append(node_target)

            # For MATCH_DIRECT_DEPS support: if this node has a direct dep on a MATCH_DIRECT_DEPS target,
            # add that target to the roots for this link group
            for dep in node.deps + node.exported_deps:
                dep_link_group = link_group_mappings.get(dep)
                if dep_link_group == MATCH_DIRECT_DEPS_LABEL:
                    roots.append(dep)
        else:
            return node.all_deps

        return None

    relevant_roots = []

    depth_first_traversal_by(
        linkable_graph_node_map,
        roots,
        partial(collect_and_traverse_roots, relevant_roots),
    )

    return relevant_roots

def _get_roots_from_mappings(spec: LinkGroupLibSpec, linkable_graph: ReducedLinkableGraph) -> (list[Label], bool):
    # Keep the graph wrapped so checking this parameter doesn't visit every node.
    roots = []
    has_empty_root = False
    for mapping in spec.group.mappings:
        # If there's no explicit root, this means we need to search the entire
        # graph to find candidate nodes.
        if not mapping.roots:
            has_empty_root = True
        elif spec.group.attrs.requires_root_node_exists:
            # If spec requires root to always exist (default True), always include to traversal to fail hard if it is not in deps.
            # Otherwise add to traversal only if we sure it is in deps graph.
            roots.extend(mapping.roots)
        else:
            roots.extend([root for root in mapping.roots if root in linkable_graph.nodes])
    return (roots, has_empty_root)

_CreatedLinkGroup = record(
    linked_object = field(LinkedObject),
    labels_to_links = field(FinalLabelsToLinks),
    linker_map_data = field([CxxLinkerMapData, None], None),
    # What the root was linked with, when the spec has a root.
    root_link_info = field(LinkInfo | None, None),
)

_CreateLinkGroupParams = record(
    category_suffix = field(str),
    anonymous = field(bool),
    allow_cache_upload = field(bool),
    error_handler = field([typing.Callable, None]),
    build_groups_context = field(BuildLinkGroupsContext),
)

def _create_link_group(
    ctx: AnalysisContext,
    spec: LinkGroupLibSpec,
    linkables: list[Label],
    linker_flags: list[typing.Any],
    params: _CreateLinkGroupParams,
    # (artifact, format) linker flags such as the group's `-Wl,--script={}`.
    script_flags: list[(Artifact, str)] = [],
) -> _CreatedLinkGroup | None:
    """
    Link a link group library, described by a `LinkGroupLibSpec`.  This is
    intended to handle regular shared libs and e.g. Python extensions.
    """

    inputs = []

    # Add extra linker flags.
    linker_info = get_cxx_toolchain_info(ctx).linker_info
    linker_type = linker_info.type
    inputs.append(
        LinkInfo(
            pre_flags = linker_flags
            +
            # An anonymous link can't carry an artifact-bearing `cmd_args` in
            # its flags, so it serializes these itself (`anon_script_flags`).
            ([] if params.anonymous else [cmd_args(artifact, format = fmt) for artifact, fmt in script_flags])
            +
            # There's scenarios where we no required syms won't be on the root
            # link lines (e.g. syms from executable), so we need to ignore
            # undefined symbols.
            get_ignore_undefined_symbols_flags(linker_type),
        )
    )

    root_link_info = None
    if spec.root != None:
        # If there's a linkable root attached to the spec, use that to guide
        # linking, as that will contain things like private linker flags that
        # might be required to link the given link group.
        root_link_info = get_link_info_from_link_infos(
            spec.root.link_infos,
            prefer_stripped = params.build_groups_context.prefer_stripped,
        )
        inputs.append(root_link_info)

    # Add roots...
    filtered_labels_to_links = get_filtered_labels_to_links_map(
        link_group = spec.group.name,
        linkables = linkables,
        is_executable_link = False,
        build_context = params.build_groups_context,
        prefer_optimized = spec.group.attrs.prefer_optimized_experimental,
    )
    inputs.extend(get_filtered_links(filtered_labels_to_links.map, params.build_groups_context.public_nodes))

    if not filtered_labels_to_links.map and not spec.root:
        # don't create empty shared libraries
        return None

    # Determine link execution preference for this link group
    # Default to "any" if not specified on the link group
    link_exec_pref = spec.group.attrs.link_execution_preference
    if link_exec_pref:
        # If link_exec_pref is an Artifact (source file), extract the basename
        # This happens when strings are specified in BUCK file attribute dictionaries
        link_exec_pref = getattr(link_exec_pref, "basename", link_exec_pref)
        link_execution_preference = LinkExecutionPreference(link_exec_pref)
    else:
        link_execution_preference = LinkExecutionPreference("any")

    link_args = [LinkArgs(infos = inputs)]
    needs_extract = hip_debug_extract_available(get_cxx_toolchain_info(ctx)) and link_args_have_hip_device_debug(link_args)
    output_name = spec.name + PRE_EXTRACT_SUFFIX if needs_extract else spec.name

    # link the rule
    link_result = cxx_link_shared_library(
        ctx = ctx,
        output = paths.join("__link_groups__", output_name),
        name = spec.name if spec.is_shared_lib else None,
        opts = link_options(
            links = link_args,
            category_suffix = params.category_suffix,
            identifier = spec.name,
            # TODO: anonymous targets cannot be used with dynamic output yet
            enable_distributed_thinlto = False if params.anonymous else spec.group.attrs.enable_distributed_thinlto,
            link_execution_preference = link_execution_preference,
            allow_cache_upload = params.allow_cache_upload,
            error_handler = params.error_handler,
        ),
        anonymous = params.anonymous,
        anon_script_flags = script_flags if params.anonymous else [],
    )
    linked_obj = link_result.linked_object
    hip_debug = collect_hip_debug_from_links(link_args)
    if hip_debug:
        linked_obj = LinkedObject(
            output = linked_obj.output,
            bitcode_bundle = linked_obj.bitcode_bundle,
            unstripped_output = linked_obj.unstripped_output,
            prebolt_output = linked_obj.prebolt_output,
            link_args = linked_obj.link_args,
            dwp = linked_obj.dwp,
            hip_arch_debug_files = hip_debug,
            external_debug_info = linked_obj.external_debug_info,
            linker_argsfile = linked_obj.linker_argsfile,
            linker_command = linked_obj.linker_command,
            index_argsfile = linked_obj.index_argsfile,
            import_library = linked_obj.import_library,
            pdb = linked_obj.pdb,
            split_debug_output = linked_obj.split_debug_output,
        )

    return _CreatedLinkGroup(
        linked_object = linked_obj,
        labels_to_links = filtered_labels_to_links,
        linker_map_data = link_result.linker_map_data,
        root_link_info = root_link_info,
    )

def _stub_library(
    ctx: AnalysisContext, name: str, extra_ldflags: list[typing.Any] = [], anonymous: bool = False, link_libraries_locally: bool = False
) -> LinkInfos:
    # When link_libraries_locally is active, explicitly request remote execution
    # for stub libraries to avoid I/O contention from hundreds of trivial local
    # links. Otherwise, use "any" to let Buck2 choose (default behavior).
    stub_execution_preference = LinkExecutionPreference("remote") if link_libraries_locally else LinkExecutionPreference("any")
    link_result = cxx_link_shared_library(
        ctx = ctx,
        output = name + ".stub",
        name = name,
        opts = link_options(
            links = [LinkArgs(flags = extra_ldflags)],
            identifier = name,
            category_suffix = "stub_library",
            link_execution_preference = stub_execution_preference,
            # Anonymous stubs are canonical (owner-independent) and tiny, so
            # sharing them through the action cache is cheap and lets other
            # owners/builds skip the link entirely.
            allow_cache_upload = anonymous,
        ),
        anonymous = anonymous,
    )
    toolchain_info = get_cxx_toolchain_info(ctx)
    linker_info = toolchain_info.linker_info
    return LinkInfos(
        # Since we link against empty stub libraries, `--as-needed` will end up
        # removing `DT_NEEDED` tags that we actually need at runtime, so pass in
        # `--no-as-needed` last to make sure this is overridden.
        # TODO(agallagher): It'd be nice to at least support a mode where we
        # don't need to use empty stub libs.
        default = wrap_with_no_as_needed_shared_libs_flags(
            linker_type = linker_info.type,
            link_info = LinkInfo(
                linkables = [SharedLibLinkable(lib = link_result.linked_object.output)],
            ),
        ),
    )

def _symbol_files_for_link_group(
    ctx: AnalysisContext, lib: LinkedObject, prefer_local: bool = False, anonymous: bool = False, hash_counter = 0
) -> (Artifact, Artifact):
    """
    Find and return all undefined and global symbols form the given library.
    """

    # Extract undefined symbols.
    undefined_symfile = extract_undefined_syms(
        ctx = ctx,
        cxx_toolchain = get_cxx_toolchain_info(ctx),
        output = lib.output,
        category_prefix = "link_groups",
        prefer_local = prefer_local,
        anonymous = anonymous,
        hash_counter = hash_counter,
    )

    # Extract global symbols.
    global_symfile = extract_global_syms(
        ctx = ctx,
        cxx_toolchain = get_cxx_toolchain_info(ctx),
        output = lib.output,
        category_prefix = "link_groups",
        prefer_local = prefer_local,
        anonymous = anonymous,
    )

    return undefined_symfile, global_symfile

def _symbol_flags_for_link_groups(
    ctx: AnalysisContext, linker_type: LinkerType, undefined_symfiles: list[Artifact] = [], global_symfiles: list[Artifact] = []
) -> list[ArgLike]:
    """
    Generate linker flags which, when applied to the main executable, make sure
    required symbols are included in the link *and* exported to the dynamic
    symbol table.
    """

    sym_linker_flags = []

    # Format undefined symbols into an argsfile with `-u`s, and add to linker
    # flags.
    sym_linker_flags.append(
        get_undefined_symbols_args(
            ctx = ctx,
            name = "undefined_symbols_args",
            symbol_files = undefined_symfiles,
            category = "link_groups_undefined_syms_args",
        ),
    )

    # Format global symfiles into a dynamic list version file, and add to
    # linker flags.
    dynamic_list_vers = create_dynamic_list_version_script(
        actions = ctx.actions,
        name = "dynamic_list.vers",
        symbol_files = global_symfiles,
        category = "link_groups_dynamic_list",
    )

    if linker_type == LinkerType("gnu"):
        sym_linker_flags.extend([
            "-Wl,--dynamic-list",
            dynamic_list_vers,
        ])

    return sym_linker_flags

def create_link_groups(
    ctx: AnalysisContext,
    public_nodes: set[Label],
    link_strategy: LinkStrategy,
    linkable_graph: ReducedLinkableGraph,
    link_groups: dict[str, Group] = {},
    link_group_specs: list[LinkGroupLibSpec] = [],
    executable_deps: list[Label] = [],
    other_roots: list[Label] = [],
    linker_flags: list[typing.Any] = [],
    prefer_stripped_objects: bool = False,
    link_group_preferred_linkage: dict[Label, Linkage] = {},
    link_group_mappings: [dict[Label, str], None] = None,
    anonymous: bool = False,
    allow_cache_upload = False,
    transformation_spec_context: TransformationSpecContext | None = None,
    error_handler: [typing.Callable, None] = None,
    # Write the link groups' `[link-groups-info]` entries with their extras (xcode data enabled).
    debug_linkable_extras: bool = False,
) -> _LinkedLinkGroups:
    # We linking libraries here so we need pic
    if link_strategy == LinkStrategy("static"):
        link_strategy = LinkStrategy("static_pic")

    # Generate stubs first, so that subsequent links can link against them.
    link_group_shared_links = {}
    specs = []
    linker_info = get_cxx_toolchain_info(ctx).linker_info
    for link_group_spec in link_group_specs:
        if should_discard_group(link_group_spec.group):
            # Don't create a link group for deps that we want to drop
            continue
        specs.append(link_group_spec)

        # always add libraries that are not constructed from link groups
        if link_group_spec.is_shared_lib:
            link_group_shared_links[link_group_spec.group.name] = _stub_library(
                ctx = ctx,
                name = link_group_spec.name,
                extra_ldflags = linker_flags,
                anonymous = anonymous,
                link_libraries_locally = linker_info.link_libraries_locally or False,
            )

    targets_consumed_by_link_groups = {}
    linked_link_groups = {}
    link_groups_debug_info = {}
    debug_entry_cache = {}
    undefined_symfiles = []
    global_symfiles = []
    roots = _find_all_relevant_roots(
        specs,
        link_group_mappings,
        executable_deps + other_roots,
        link_strategy,
        linkable_graph,
    )

    pic_behavior = get_cxx_toolchain_info(ctx).pic_behavior
    if pic_behavior == PicBehavior("supported"):
        pic_behavior = PicBehavior("always_enabled")
    linkables = _collect_all_linkables(
        linkable_graph = linkable_graph,
        is_executable_link = False,
        link_strategy = link_strategy,
        link_group_preferred_linkage = link_group_preferred_linkage,
        pic_behavior = pic_behavior,
        link_group_roots = roots,
    )

    link_group_libs = {name: (None, lib) for name, lib in link_group_shared_links.items()}

    # All groups share these mappings, so checking for MATCH_DIRECT_DEPS once is sufficient.
    may_have_match_direct_deps = False
    if link_group_mappings:
        for target in link_group_mappings:
            if link_group_mappings[target] == MATCH_DIRECT_DEPS_LABEL:
                may_have_match_direct_deps = True
                break

    build_groups_context = BuildLinkGroupsContext(
        public_nodes = public_nodes,
        linkable_graph = linkable_graph,
        link_groups = link_groups,
        link_group_mappings = link_group_mappings,
        link_strategy = link_strategy,
        link_group_preferred_linkage = link_group_preferred_linkage,
        pic_behavior = get_cxx_toolchain_info(ctx).pic_behavior,
        link_group_libs = link_group_libs,
        # TODO(agallagher): Should we support alternate link strategies
        # (e.g. bottom-up with symbol errors)?
        link_group_roots = {},
        may_have_match_direct_deps = may_have_match_direct_deps,
        prefer_stripped = prefer_stripped_objects,
        transformation_spec_context = transformation_spec_context,
    )

    create_link_group_params = _CreateLinkGroupParams(
        category_suffix = "link_group",
        anonymous = anonymous,
        allow_cache_upload = allow_cache_upload,
        error_handler = error_handler,
        build_groups_context = build_groups_context,
    )

    toolchain_info = get_cxx_toolchain_info(ctx)
    linker_info = toolchain_info.linker_info

    for counter, link_group_spec in enumerate(specs):
        # NOTE(agallagher): It might make sense to move this down to be
        # done when we generated the links for the executable, so we can
        # handle the case when a link group can depend on the executable.
        created_link_group = _create_link_group(
            ctx = ctx,
            spec = link_group_spec,
            linkables = linkables[link_group_spec.group.name],
            linker_flags = (linker_flags + link_group_spec.group.attrs.linker_flags + link_group_spec.group.attrs.exported_linker_flags),
            script_flags = (
                ([(link_group_spec.group.attrs.linker_script, "-Wl,--script={}")] if link_group_spec.group.attrs.linker_script else [])
                + ([(link_group_spec.group.attrs.version_script, "-Wl,--version-script={}")] if link_group_spec.group.attrs.version_script else [])
            ),
            params = create_link_group_params,
        )

        if created_link_group == None:
            # the link group did not match anything, don't create shlib interface
            continue

        link_group_lib = created_link_group.linked_object

        root_label = link_group_spec.root.label if link_group_spec.root else None
        link_groups_debug_info[link_group_spec.name] = LinkGroupsDebugLinkableItem(
            ordered_linkables = create_debug_linkable_entries(
                created_link_group.labels_to_links.map,
                root_label,
                debug_linkable_extras,
                root_link_info = created_link_group.root_link_info,
                entry_cache = debug_entry_cache,
                prefer_optimized = link_group_spec.group.attrs.prefer_optimized_experimental,
            ),
        )

        for linked_target, link_info in created_link_group.labels_to_links.map.items():
            if link_info.output_style != LibOutputStyle("shared_lib"):
                # Remember all targets that were statically linked into link group
                targets_consumed_by_link_groups[linked_target] = link_group_spec.group.name

        if link_group_spec.root:
            # If link group has root it always being linked statically
            targets_consumed_by_link_groups[link_group_spec.root.label] = link_group_spec.group.name

        # On GNU, use shlib interfaces when the tool is available.
        if cxx_is_gnu(ctx) and get_cxx_toolchain_info(ctx).linker_info.mk_shlib_intf != None:
            shlib_for_link = shared_library_interface(
                ctx = ctx,
                shared_lib = link_group_lib.output,
                anonymous = anonymous,
            )
        else:
            shlib_for_link = link_group_lib.output

        link_info = wrap_link_info(
            LinkInfo(
                linkables = [SharedLibLinkable(lib = shlib_for_link)],
            ),
            pre_flags = link_group_spec.group.attrs.exported_linker_flags,
        )

        if link_group_spec.group.attrs.no_as_needed:
            link_info = wrap_with_no_as_needed_shared_libs_flags(
                linker_type = linker_info.type,
                link_info = link_info,
            )

        linked_link_groups[link_group_spec.group.name] = _LinkedLinkGroup(
            artifact = link_group_lib,
            library = None
            if not link_group_spec.is_shared_lib
            else LinkGroupLib(
                shared_libs = SharedLibraries(
                    libraries = [
                        create_shlib(
                            label = link_group_spec.label or ctx.label,
                            soname = link_group_spec.name,
                            lib = link_group_lib,
                        ),
                    ],
                ),
                shared_link_infos = LinkInfos(
                    default = link_info,
                ),
            ),
            linker_map_data = created_link_group.linker_map_data,
        )

        # Merge and format all symbol files into flags that we can pass into
        # the binary link.
        undefined_symfile, global_symfile = _symbol_files_for_link_group(
            ctx = ctx,
            lib = link_group_lib,
            anonymous = anonymous,
            hash_counter = counter,
        )
        undefined_symfiles.append(undefined_symfile)
        global_symfiles.append(global_symfile)

    # Add linker flags to make sure any symbols from the main executable
    # needed by these link groups are pulled in and exported to the dynamic
    # symbol table.
    symbol_ldflags = []
    if linked_link_groups:
        symbol_ldflags.extend(
            _symbol_flags_for_link_groups(
                ctx = ctx,
                linker_type = linker_info.type,
                undefined_symfiles = undefined_symfiles,
                global_symfiles = global_symfiles,
            ),
        )

    return _LinkedLinkGroups(
        libs = linked_link_groups,
        symbol_ldflags = symbol_ldflags,
        libs_debug_info = link_groups_debug_info,
        targets_consumed_by_link_groups = targets_consumed_by_link_groups,
    )

def get_transitive_deps_matching_labels(linkable_graph_node_map: dict[Label, LinkableNode], roots: list[Label], label: str) -> list[Label]:
    # NOTE: Our Haskell DLL support impl currently links transitive haskell
    # deps needed by DLLs which get linked into the main executable as link-
    # whole.  To emulate this, we mark Haskell rules with a special label
    # and traverse this to find all the nodes we need to link whole.
    nodes = []
    for dep in get_transitive_deps(linkable_graph_node_map, roots):
        linkable = linkable_graph_node_map[dep]
        if label not in linkable.labels:
            continue
        nodes.append(dep)
    return nodes

def build_shared_libs_for_symlink_tree(
    use_link_groups: bool,
    link_group_ctx: LinkGroupContext,
    link_strategy: LinkStrategy,
    shared_libraries: list[SharedLibrary],
    extra_shared_libraries: list[SharedLibrary],
) -> list[SharedLibrary]:
    # Which targets we actually materialized as symlinks to link group
    added_link_group_symlinks_libs = set()
    symlink_tree_shared_libraries = []

    def is_shlib_added(soname: Soname) -> bool:
        return soname.is_str and soname.ensure_str() in added_link_group_symlinks_libs

    def add_shib(shlib: SharedLibrary):
        if shlib.soname.is_str:
            added_link_group_symlinks_libs.add(shlib.soname.ensure_str())
        symlink_tree_shared_libraries.append(shlib)

    if use_link_groups:
        # When there are no matches for a pattern based link group,
        # `link_group_mappings` will not have an entry associated with the lib.
        for _name, link_group_lib in link_group_ctx.link_group_libs.items():
            for link_group_shlib in link_group_lib.shared_libs.libraries:
                add_shib(link_group_shlib)

    for shlib in shared_libraries:
        if is_shlib_added(shlib.soname):
            # Shlib was already materialised as link group.
            # This may happen if link group spec had this target
            # as root. That will produce link group with exact
            # .so file and dynamic linker will be satisfied.
            continue

        if link_strategy == LinkStrategy("shared") and shlib.label in link_group_ctx.targets_consumed_by_link_groups:
            link_group_link = create_link_group_link(
                link_group_ctx.link_group_libs[link_group_ctx.targets_consumed_by_link_groups[shlib.label]],
                shlib,
            )
            add_shib(link_group_link)

        elif not use_link_groups or is_link_group_shlib(shlib.label, link_group_ctx) or link_strategy == LinkStrategy("shared"):
            add_shib(shlib)

    # Add in extra, rule-specific shared libs.
    for extra_shlib in extra_shared_libraries:
        if not is_shlib_added(extra_shlib.soname):
            add_shib(extra_shlib)

    return symlink_tree_shared_libraries

def create_link_group_link(link_group_lib: LinkGroupLib, consumed_library: SharedLibrary) -> SharedLibrary:
    """
    This method implements symlinking from original .so to link group .so
    for link groups in **dynamic linking**.
    Current problem is: with following setup
    ```
        :bin
       |    |
      :A   :C
       |    │
       └ :B ┘
    ```

    If we put `:A` and `:B` to link group, `lib_c.so` will still add `lib_b.so` to `NEEDS` section.
    But `lib_b.so` is gonna be grouped to `lib_a_b_lg.so` and there is no way to propagate this information to `lib_c.so`.
    But we actually can have "stubs" for `lib_a.so` and `lib_b.so` that all point to actual `lib_a_b_lg.so`.
    This approach satisfies dynamic linker.
    """

    if len(link_group_lib.shared_libs.libraries) != 1:
        fail("This method should only be used with auto link groups that produce exactly one shared libray")
    link_group_shlib = link_group_lib.shared_libs.libraries[0]

    return create_shlib(
        lib = link_group_shlib.lib,
        link_args = link_group_shlib.link_args,
        shlib_deps = link_group_shlib.shlib_deps,
        can_be_asset = link_group_shlib.can_be_asset,
        soname = consumed_library.soname,  # <=== we match original target soname that will symlink to link group
        label = consumed_library.label,
    )
