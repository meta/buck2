# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//cxx:groups.bzl",
    "get_roots_from_mapping",
    "make_info_subtarget_providers",
    "parse_groups_definitions",
)
load(
    "@prelude//cxx:link_groups.bzl",
    "build_link_group_info",
)
load(
    "@prelude//cxx:link_groups_types.bzl",
    "link_group_inlined_map_attr",
)
load(
    "@prelude//linking:link_groups.bzl",
    "LinkGroupLibInfo",
)
load(
    "@prelude//linking:link_info.bzl",
    "MergedLinkInfo",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "LinkableGraph",
    "create_linkable_graph",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
)
load("@prelude//user:rule_spec.bzl", "RuleRegistrationSpec")
load("@prelude//utils:utils.bzl", "flatten")

def _impl(ctx: AnalysisContext) -> list[Provider]:
    # Extract graphs from the roots via the raw attrs, as `parse_groups_definitions`
    # parses them as labels.

    deps = flatten([get_roots_from_mapping(mapping) for entry in ctx.attrs.map for mapping in entry[1]])
    linkable_graph = create_linkable_graph(
        ctx,
        deps = [dep[LinkableGraph] for dep in deps],
    )
    link_groups = parse_groups_definitions(ctx.attrs.map, lambda root: root.label)
    link_group_info = build_link_group_info(linkable_graph, link_groups)
    return [
        DefaultInfo(
            sub_targets = {
                "info": make_info_subtarget_providers(ctx, link_group_info.groups.values(), link_group_info.mappings),
            }
        ),
        link_group_info,
    ]

registration_spec = RuleRegistrationSpec(
    name = "link_group_map",
    impl = _impl,
    attrs = {
        "map": link_group_inlined_map_attr(
            root_attr = attrs.dep(
                providers = [
                    LinkGroupLibInfo,
                    LinkableGraph,
                    MergedLinkInfo,
                    SharedLibraryInfo,
                ],
            ),
        ),
    },
)
