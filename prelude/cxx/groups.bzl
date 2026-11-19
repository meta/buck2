# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:groups_types.bzl", "Traversal")
load("@prelude//linking:types.bzl", "Linkage")
load(
    "@prelude//utils:build_target_pattern.bzl",
    "BuildTargetPattern",
    "parse_build_target_pattern",
)
load("@prelude//utils:expect.bzl", "expect")
load(
    "@prelude//utils:graph_utils.bzl",
    "depth_first_traversal_by",
)
load(
    "@prelude//utils:strings.bzl",
    "strip_prefix",
)
load(
    "@prelude//utils:utils.bzl",
    "value_or",
)
load(
    ":groups_types.bzl",
    "Group",
    "GroupAttrs",
    "GroupDefinition",
    "GroupFilterInfo",
    "GroupMapping",
)

_VALID_ATTRS = [
    "enable_distributed_thinlto",
    "enable_if_node_count_exceeds",
    "discard_group",
    "linker_flags",
    "linker_script",
    "version_script",
    "exported_linker_flags",
    "link_execution_preference",
    "no_as_needed",
    "requires_root_node_exists",
    "prohibit_file_duplicates",
    "prefer_optimized_experimental",
]

# Traversal types in this list will only assign the node
# to a target (as opposed to the transitive deps of the node's tree).
_TRAVERSALS_TO_ASSIGN_NODE = [
    Traversal("node"),
    Traversal("subfolders"),
    # TODO (dust): Possible perf optimization:
    # When intersecting configured targets, it's not possible to intersect
    # a parent without also intersecting it's children.
    #
    # As a result, there's a possible perf optimization to assign 'tree'
    # to intersected targets instead, and leverage that to avoid traversing
    # the entire tree of every root.
    #
    # For example:
    # If iterating the tree of 'root2' we find a node which
    # was also present in 'root1', we can skip traversing the subtree
    # because it's evitable that everything is going to match there too.
    Traversal("intersect_any_roots"),
]

# Creates a group from an existing group, overwriting any properties provided
def create_group(
    group: Group,
    name: [None, str] = None,
    mappings: [None, list[GroupMapping]] = None,
    attrs: [None, GroupAttrs] = None,
    definition_type: [None, GroupDefinition] = None,
):
    return Group(
        name = value_or(name, group.name),
        mappings = value_or(mappings, group.mappings),
        attrs = value_or(attrs, group.attrs),
        definition_type = value_or(definition_type, group.definition_type),
    )

def get_roots_from_mapping(mapping):
    deps = mapping[0] if type(mapping[0]) == "list" else [mapping[0]]
    return filter(None, deps)

def parse_groups_definitions(
    map: list,
    # Function to parse a root label from the input type, allowing different
    # callers to have different top-level types for the `root`s.
    parse_root: typing.Callable = lambda d: d,
) -> list[Group]:
    groups = []
    group_names = set()
    for map_entry in map:
        name = map_entry[0]

        # Dedup the link group specs, take the deinition from the first definition
        if name in group_names:
            continue
        group_names.add(name)
        mappings = map_entry[1]
        attrs = (map_entry[2] or {}) if len(map_entry) > 2 else {}

        for attr in attrs:
            if attr not in _VALID_ATTRS:
                fail("invalid attr '{}' for link group '{}' found. Valid attributes are {}.".format(attr, name, _VALID_ATTRS))
        group_attrs = GroupAttrs(
            enable_distributed_thinlto = attrs.get("enable_distributed_thinlto", False),
            enable_if_node_count_exceeds = attrs.get("enable_if_node_count_exceeds", None),
            discard_group = attrs.get("discard_group", False),
            linker_flags = attrs.get("linker_flags", []),
            linker_script = attrs.get("linker_script", None),
            version_script = attrs.get("version_script", None),
            exported_linker_flags = attrs.get("exported_linker_flags", []),
            link_execution_preference = attrs.get("link_execution_preference", None),
            no_as_needed = attrs.get("no_as_needed", False),
            requires_root_node_exists = attrs.get("requires_root_node_exists", True),
            prohibit_file_duplicates = attrs.get("prohibit_file_duplicates", False),
            prefer_optimized_experimental = attrs.get("prefer_optimized_experimental", False),
        )

        parsed_mappings = []
        for entry in mappings:
            traversal = _parse_traversal_from_mapping(entry[1])
            mapping = GroupMapping(
                roots = filter(None, [parse_root(root) for root in get_roots_from_mapping(entry)]),
                traversal = traversal,
                filters = _parse_filter_from_mapping(entry[2]),
                preferred_linkage = Linkage(entry[3]) if len(entry) > 3 and entry[3] else None,
            )
            num_roots = len(mapping.roots) if mapping.roots else 0
            if num_roots > 1 and mapping.traversal != Traversal("intersect_any_roots"):
                fail("Invariant. A link_group mapping with traversal type: {} can only have 1 root node. {} found.".format(mapping.traversal, mapping.roots))
            elif mapping.traversal == Traversal("intersect_any_roots") and num_roots < 2:
                fail("Invariant. A link_group mapping with traversal type 'intersect' must have at least 2 root nodes. {} found.".format(mapping.roots))

            parsed_mappings.append(mapping)

        group = Group(
            name = name,
            mappings = parsed_mappings,
            attrs = group_attrs,
            definition_type = GroupDefinition("explicit"),
        )
        groups.append(group)

    return groups

def _parse_traversal_from_mapping(entry: str) -> Traversal:
    if entry == "tree":
        return Traversal("tree")
    elif entry == "node":
        return Traversal("node")
    elif entry == "subfolders":
        return Traversal("subfolders")
    elif entry == "intersect_any_roots":
        return Traversal("intersect_any_roots")
    else:
        fail("Unrecognized group traversal type: " + entry)

def _parse_filter(entry: str) -> GroupFilterInfo:
    for prefix in ("label:", "tag:"):
        label_regex = strip_prefix(prefix, entry)
        if label_regex != None:
            # We need the anchors "^"" and "$" because experimental_regex match
            # anywhere in the text, while we want full text match for group label
            # text.
            regex_expr = regex("^{}$".format(label_regex), fancy = False)

            def matches_regex(_r, _t, labels):
                return regex_expr.any_match(labels)

            return GroupFilterInfo(
                matches = matches_regex,
                info = {"regex": str(regex_expr)},
            )

        target_regex = strip_prefix("target_regex:", entry)
        if target_regex != None:
            regex_expr = regex("^{}$".format(target_regex), fancy = True)

            def matches_regex(_r, t, _labels):
                return regex_expr.match(str(t.raw_target()))

            return GroupFilterInfo(
                matches = matches_regex,
                info = {"target_regex": str(regex_expr)},
            )

    pattern = strip_prefix("pattern:", entry)
    if pattern != None:
        build_target_pattern = parse_build_target_pattern(pattern)
        pattern_matches = build_target_pattern.matches

        def matches_target_pattern(_r, t, _labels):
            return pattern_matches(t)

        return GroupFilterInfo(
            matches = matches_target_pattern,
            info = _make_json_info_for_build_target_pattern(build_target_pattern),
        )

    fail("Invalid group mapping filter: {}\nFilter must begin with `label:`, `tag:`, `target_regex` or `pattern:`.".format(entry))

def _parse_filter_from_mapping(entry: [list[str | Dependency], str, Dependency, None]) -> list[GroupFilterInfo]:
    if type(entry) == type([]):
        return [_parse_filter(e) if isinstance(e, str) else e[GroupFilterInfo] for e in entry]
    elif isinstance(entry, Dependency):
        return [entry[GroupFilterInfo]]
    elif type(entry) == type(""):
        return [_parse_filter(entry)]
    return []

def compute_mappings(groups_map: dict[str, Group], graph_map: dict[Label, typing.Any]) -> dict[Label, str]:
    """
    Returns the group mappings {target label -> group name} based on the provided groups_map and graph.
    """
    if not groups_map:
        return {}

    target_to_group_map = {}
    node_traversed_targets = {}

    group_mappings_with_roots = []
    group_mappings_without_roots = []
    group_to_targets = {}

    groups = groups_map.values()
    for group in groups:
        for mapping in group.mappings:
            if mapping and not mapping.roots:
                group_mappings_without_roots.append((group, mapping))
            else:
                group_mappings_with_roots.append((group, mapping))

    # We assume there's more targets in the build graph then there are groups
    for target in graph_map:
        graph_node_labels = graph_map[target].labels
        for group, mapping in group_mappings_without_roots:
            all_match = True
            for filter in mapping.filters:
                if not filter.matches(None, target, graph_node_labels):
                    all_match = False
                    break
            if all_match:
                group_to_targets.setdefault(group.name, []).append((target, mapping))

    # The children of a node, `deps + exported_deps`, concatenated when a walk
    # first reaches the node and shared by every walk.
    children_of = {}

    # The rooted mappings with filters all walk the graph from their roots, so
    # one pass finds the targets of most of them. The targets are then claimed
    # in the order of the mappings, as `_update_target_to_group_mapping` gives
    # a target to the first mapping that claims it.
    walked = [mapping for _, mapping in group_mappings_with_roots if _is_walked(mapping)]
    walked_targets = _find_targets_in_mappings(graph_map, children_of, walked) if walked else []
    next_walked = 0
    for group, mapping in group_mappings_with_roots:
        if _is_walked(mapping):
            targets_in_mapping = walked_targets[next_walked]
            next_walked += 1
        else:
            targets_in_mapping = _find_targets_in_mapping(graph_map, mapping, children_of)
        for target in targets_in_mapping:
            # If the target doesn't exist in our graph, skip the mapping.
            if target not in graph_map:
                continue
            group_to_targets.setdefault(group.name, []).append((target, mapping))

    for group in groups_map.values():
        for target, mapping in group_to_targets.get(group.name, []):
            _update_target_to_group_mapping(graph_map, target_to_group_map, node_traversed_targets, group, groups_map, mapping, target)

    return target_to_group_map

def get_dedupped_roots_from_groups(groups: list[Group]) -> list[Label]:
    roots = {}
    for group in groups:
        for mapping in group.mappings:
            if not mapping.roots:
                continue

            for root in mapping.roots:
                roots[root] = True

    return list(roots.keys())

def _is_walked(mapping: GroupMapping) -> bool:
    """
    Whether `_find_targets_in_mappings` finds the mapping's targets: it does
    for the rooted mappings with filters, except the `intersect_any_roots`
    ones, which `_find_targets_in_mapping` walks root by root, and the
    `subfolders` ones, which create their implicit groups in the order of
    their targets.
    """
    return bool(mapping.filters) and bool(mapping.roots) and mapping.traversal not in [Traversal("intersect_any_roots"), Traversal("subfolders")]

# Types removed to avoid unnecessary type checking which degrades performance.
def _find_targets_in_mappings(graph_map, children_of, mappings):
    """
    For each of `mappings`, which all have roots and filters, the nodes its
    walk from its roots reaches that pass its filters: the same nodes
    `_matching_targets_under` finds, in topological rather than depth first
    order.

    A walk goes on below a node when the node matches, unless the traversal
    is `tree`, or when it does not match and the failing filter does not stop
    the walk; so whether a walk reaches a node depends only on the graph, and
    one pass over the nodes reachable from any root in topological order (the
    graph is a DAG, being part of the configured target graph) finds the
    targets of every mapping at once, with the mappings reaching a node kept
    as a bit mask. The pass visits every node reachable from the roots, also
    the ones no walk reaches.
    """
    roots = {}
    seeds = {}
    for i, mapping in enumerate(mappings):
        bit = 1 << i
        for root in mapping.roots:
            if root not in graph_map:
                fail("Expected node {} in graph nodes".format(root))
            roots[root] = None
            seeds[root] = seeds.get(root, 0) | bit

    # Reverse post order of the nodes reachable from any root: parents before children.
    visited = set()
    post_order = []
    stack = [(root, False) for root in roots]
    for _ in range(2000000000):
        if not stack:
            break
        node, done = stack.pop()
        if done:
            post_order.append(node)
            continue
        if node in visited:
            continue
        visited.add(node)
        stack.append((node, True))
        children = children_of.get(node)
        if children == None:
            graph_node = graph_map.get(node)
            if graph_node == None:
                fail("Expected node {} in graph nodes".format(node))
            children = graph_node.deps + graph_node.exported_deps
            children_of[node] = children
        for child in children:
            if child not in visited:
                stack.append((child, False))
    expect(not stack, "Expected to be done with graph traversal stack.")

    walks = []
    for i, mapping in enumerate(mappings):
        root = mapping.roots[0] if len(mapping.roots) == 1 else None
        filters = mapping.filters
        single = filters[0].matches if len(filters) == 1 else None
        traverse_on_miss = not filters[0].stop_at_first_non_match if single != None else None
        walks.append((1 << i, root, mapping.traversal == Traversal("tree"), single, traverse_on_miss, filters))
    results = [[] for _ in mappings]

    alive = dict(seeds)
    for idx in range(len(post_order) - 1, -1, -1):
        node = post_order[idx]
        mask = alive.get(node)
        if not mask:
            continue
        labels = graph_map[node].labels
        pass_mask = 0
        for j in range(len(walks)):
            bit, root, stop_at_match, single, traverse_on_miss, filters = walks[j]
            if not mask & bit:
                continue
            if single != None:
                if single(root, node, labels):
                    results[j].append(node)
                    if stop_at_match:
                        continue
                elif not traverse_on_miss:
                    continue
            else:
                matched, traverse = _matches_filters(filters, root, node, labels)
                if matched:
                    results[j].append(node)
                    if stop_at_match:
                        continue
                elif not traverse:
                    continue
            pass_mask |= bit
        if pass_mask:
            for child in children_of[node]:
                alive[child] = alive.get(child, 0) | pass_mask

    return results

# Types removed to avoid unnecessary type checking which degrades performance.
def _find_targets_in_mapping(
    graph_map,  # dict[Label, typing.Any]
    mapping,  # GroupMapping
    children_of,
):  # dict[Label, list[Label]]
    # Returns list[Label].
    # If we have no filtering, we don't need to do any traversal to find targets to include.
    if not mapping.filters:
        if not mapping.roots:
            # Some link groups may want to partially define their mapping roots based on constraint
            # that potentially can be resolved to `None`.
            #
            # E.g:
            # ```
            # ("evict-mkl", [
            #   (":mkl_ilp64_omp", "node", None, "shared"),
            #   (select(
            #     {"DEFAULT": None, "ovr_config//runtime:platform010": "//IntelComposerXE:mkl_ilp64_omp" }),
            #     "node", None, "shared"
            #   ),
            # ])
            # ```
            # Second mapping will be resolved to `(None, "node", None, "shared")` and will not handle anything.
            # There is no convenient way to gracefully handle that in user-facing link groups API.
            return []

        elif mapping.traversal != Traversal("intersect_any_roots"):
            return mapping.roots

    # Else find all dependencies that match the filter.
    filters = mapping.filters
    stop_at_match = mapping.traversal == Traversal("tree")

    if not mapping.roots:
        matching_targets = {}
        for node in graph_map:
            matched, _ = _matches_filters(filters, None, node, graph_map[node].labels)
            if matched:
                matching_targets[node] = None
        return matching_targets.keys()
    elif mapping.traversal == Traversal("intersect_any_roots"):
        targets_to_counter = {}
        for root in mapping.roots:
            for t in _matching_targets_under(graph_map, children_of, [root], root, filters, stop_at_match):
                targets_to_counter[t] = targets_to_counter.get(t, 0) + 1

        return [t for t, count in targets_to_counter.items() if count > 1]
    else:
        # When we have multiple roots there is no way to determine which
        # root is being traversed, so just set it to None.
        root = mapping.roots[0] if len(mapping.roots) == 1 else None
        return _matching_targets_under(graph_map, children_of, mapping.roots, root, filters, stop_at_match)

def _matches_filters(filters, root, node, labels):  # (list[GroupFilterInfo], Label | None, Label, list[str]) -> (bool, bool)
    """
    Whether `node` passes every filter, and whether the traversal continues
    below it when it does not: a filter can ask to stop at the first non-match.
    """
    for filter in filters:
        if not filter.matches(root, node, labels):
            return (False, not filter.stop_at_first_non_match)
    return (True, True)

def _matching_targets_under(
    graph_map,  # dict[Label, typing.Any]
    children_of,  # dict[Label, list[Label]]
    roots,  # list[Label]
    root,  # Label | None, passed to the filters
    filters,  # list[GroupFilterInfo]
    stop_at_match,
):  # bool
    """
    The nodes under `roots` that pass `filters`, in the order a preorder
    right-to-left depth first traversal (see `depth_first_traversal_by`)
    reaches them. With `stop_at_match` (the `tree` traversal) a matching
    node's children are not traversed, although they may still be reached
    through other nodes.
    """
    matching_targets = {}
    visited = set(roots)
    stack = list(visited)

    for _ in range(len(graph_map)):
        if not stack:
            break
        node = stack.pop()
        graph_node = graph_map.get(node)
        if graph_node == None:
            fail("Expected node {} in graph nodes".format(node))

        matched, traverse = _matches_filters(filters, root, node, graph_node.labels)
        if matched:
            matching_targets[node] = None
            traverse = not stop_at_match
        if traverse:
            children = children_of.get(node)
            if children == None:
                children = graph_node.deps + graph_node.exported_deps
                children_of[node] = children
            for child in children:
                if child not in visited:
                    visited.add(child)
                    stack.append(child)

    expect(not stack, "Expected to be done with graph traversal stack.")

    return matching_targets.keys()

# Extracted from `_update_target_to_group_mapping` to avoid function allocations inside the loop
def _assign_target_to_group(
    target_to_group_map,  #: {"label": str}
    node_traversed_targets,  #: {"label": None}
    group,  #  Group,
    groups_map,  # {str: Group}
    mapping,  # GroupMapping
    target,  # Label
    node_traversal,
):  # bool
    # If the target hasn't already been assigned to a group, assign it to the
    # first group claiming the target. Return whether the target was already assigned.
    if target not in target_to_group_map:
        if mapping.traversal == Traversal("subfolders"):
            generated_group_name = _generate_group_subfolder_name(group.name, target.package)
            _add_to_implicit_link_group(generated_group_name, group, groups_map, target_to_group_map, target)
        else:
            target_to_group_map[target] = group.name

        if node_traversal:
            node_traversed_targets[target] = None
        return False
    else:
        return True

# Extracted from `_update_target_to_group_mapping` to avoid function allocations inside the loop
def _transitively_add_targets_to_group_mapping(
    assign_target_to_group,  # (Label, bool) -> bool
    node_traversed_targets,  #: {"label": None}
    graph_map,  # {"label": "_b"}
    node,
):  # ([Label]) -> None
    previously_processed = assign_target_to_group(node, False)

    # If the node has been previously processed, and it was via tree (not node), all child nodes have been assigned
    if previously_processed and node not in node_traversed_targets:
        return None
    graph_node = graph_map[node]
    return graph_node.deps + graph_node.exported_deps

# Types removed to avoid unnecessary type checking which degrades performance.
def _update_target_to_group_mapping(
    graph_map,  # {"label": "_b"}
    target_to_group_map,  #: {"label": str}
    node_traversed_targets,  #: {"label": None}
    group,  #  Group,
    groups_map,  # {str: Group}
    mapping,  # GroupMapping
    target,
):  # Label
    assign_target_to_group = partial(_assign_target_to_group, target_to_group_map, node_traversed_targets, group, groups_map, mapping)  # (Label, bool) -> bool
    transitively_add_targets_to_group_mapping = partial(
        _transitively_add_targets_to_group_mapping, assign_target_to_group, node_traversed_targets, graph_map
    )  # (Label) -> list[Label]

    if mapping.traversal in _TRAVERSALS_TO_ASSIGN_NODE:
        assign_target_to_group(target, True)
    else:  # tree
        depth_first_traversal_by(graph_map, [target], transitively_add_targets_to_group_mapping)

def _add_to_implicit_link_group(
    generated_group_name,  # str
    group,  # Group
    groups_map,  # {str: Group}
    target_to_group_map,  # {Label: str}
    target,
):  # Label
    target_to_group_map[target] = generated_group_name
    if generated_group_name not in groups_map:
        groups_map[generated_group_name] = create_group(
            group = group,
            name = generated_group_name,
            definition_type = GroupDefinition("implicit"),
        )
    elif groups_map[generated_group_name].definition_type == GroupDefinition("explicit"):
        hashed_group_name = _hash_group_name(group.name, generated_group_name)
        _add_to_implicit_link_group(hashed_group_name, group, groups_map, target_to_group_map, target)

def _generate_group_subfolder_name(
    group,  # str,
    package,
):  # str
    """Dynamically generating link group name for "subfolders" traversal."""
    name = group + "_" + package.replace("/", "_")

    if len(name) > 246:
        # Maximum filename size in unix is 255.
        # We prefix all libraries with "lib" (3 symbols) and suffix with ".dylib" (6 symbols) or ".so" (3 symbols).
        # Assuming ".dylib" suffix cause it's the longest, we can allow (255 - 3 - 6) = 246 symbols for the rest of the name.
        name = _hash_group_name(group, name)
    return name

def _hash_group_name(prefix: str, name: str) -> str:
    """
    Creates new name via simple hashing.
    Hash algorithm is stable in starlark: https://fburl.com/code/ptegkov6
    """
    return "{}_{}".format(prefix, str(hash(name)))

def _make_json_info_for_build_target_pattern(build_target_pattern: BuildTargetPattern) -> dict[str, typing.Any]:
    # `BuildTargetPattern` contains lambdas which are not serializable, so
    # have to generate the JSON representation
    return {
        "cell": build_target_pattern.cell,
        "kind": build_target_pattern.kind,
        "name": build_target_pattern.name,
        "path": build_target_pattern.path,
    }

def _make_json_info_for_group_mapping(group_mapping: GroupMapping) -> dict[str, typing.Any]:
    return {
        "filters": [filter.info for filter in group_mapping.filters],
        "preferred_linkage": group_mapping.preferred_linkage,
        "roots": group_mapping.roots,
        "traversal": group_mapping.traversal,
    }

def _make_json_info_for_group(group: Group) -> dict[str, typing.Any]:
    return {
        "attrs": group.attrs,
        "mappings": [_make_json_info_for_group_mapping(mapping) for mapping in group.mappings],
        "name": group.name,
    }

def make_info_subtarget_providers(ctx: AnalysisContext, groups: list[Group], mappings: dict[Label, str]) -> list[Provider]:
    info_json = {
        "groups": {group.name: _make_json_info_for_group(group) for group in groups},
        "mappings": mappings,
    }
    json_output = ctx.actions.write_json("link_group_map_info.json", info_json, pretty = True, has_content_based_path = False)
    return [DefaultInfo(default_output = json_output)]
