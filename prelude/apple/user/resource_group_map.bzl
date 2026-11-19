# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//apple:resource_groups.bzl",
    "ResourceGraphNode",  # @unused Used as a type
    "ResourceGroupInfo",
    "create_resource_graph",
    "get_resource_graph_node_map_func",
)
load(
    "@prelude//cxx:groups.bzl",
    "compute_mappings",
    "create_group",
    "get_roots_from_mapping",
    "make_info_subtarget_providers",
    "parse_groups_definitions",
)
load(
    "@prelude//cxx:groups_types.bzl",
    "GroupMapping",  # @unused Used as a type
)
load("@prelude//utils:utils.bzl", "flatten")

def resource_group_map_impl(ctx: AnalysisContext) -> list[Provider]:
    resource_groups = parse_groups_definitions(ctx.attrs.map, lambda root: root.label)

    resource_group_to_implicit_deps_mapping = {group: flatten([get_roots_from_mapping(mapping) for mapping in mappings]) for group, mappings in ctx.attrs.map}
    flattend_resource_group_deps = flatten(resource_group_to_implicit_deps_mapping.values())

    resource_graph = create_resource_graph(
        ctx = ctx,
        labels = [],
        deps = flattend_resource_group_deps,
        exported_deps = [],
    )
    resource_graph_node_map = get_resource_graph_node_map_func(resource_graph)()
    mappings = compute_mappings(
        groups_map = {
            group.name: create_group(
                group = group,
                # User provided mappings may contain entries that don't support
                # ResourceGraphInfo, which `create_resource_graph` removes above.
                # So make sure we remove them from the mappings too, otherwise
                # `compute_mappings` crashes on the inconsistency.
                mappings = filter(
                    None,
                    [_fixup_mapping_to_only_include_roots_in_the_map(m, resource_graph_node_map) for m in group.mappings],
                ),
            )
            for group in resource_groups
        },
        graph_map = resource_graph_node_map,
    )
    return [
        DefaultInfo(
            sub_targets = {
                "info": make_info_subtarget_providers(ctx, resource_groups, mappings),
            }
        ),
        ResourceGroupInfo(
            groups = resource_groups,
            groups_hash = hash(str(resource_groups)),
            mappings = mappings,
            # The consumer of this info may not have deps that cover that labels
            # referenced in our roots, so propagate them here.
            # NOTE(agallagher): We do this to maintain existing behavior here
            # but it's not clear if it's actually desirable behavior.
            resource_group_to_implicit_deps_mapping = resource_group_to_implicit_deps_mapping,
        ),
    ]

def _fixup_mapping_to_only_include_roots_in_the_map(mapping: GroupMapping, node_map: dict[Label, ResourceGraphNode]) -> GroupMapping | None:
    if not mapping.roots:
        return mapping

    filtered_roots = [root for root in mapping.roots if root in node_map]
    if not filtered_roots:
        return None

    return GroupMapping(
        roots = filtered_roots,
        traversal = mapping.traversal,
        filters = mapping.filters,
        preferred_linkage = mapping.preferred_linkage,
    )
