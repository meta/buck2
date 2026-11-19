# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//linking:link_info.bzl",
    "LibOutputStyle",
    "LinkStrategy",
)
load("@prelude//linking:types.bzl", "Linkage")
load(":groups_types.bzl", "Group", "GroupFilterInfo", "Traversal")

# What a project generator needs to reproduce the link, carried by an entry
# when xcode data is enabled (`cxx.enable_xcode_data`).
LinkGroupsDebugLinkableEntryExtras = record(
    # The target the entry stands for: a target entry's own target; for a link
    # group entry the target owning the group's library, None when the link
    # created the group itself (auto link groups).
    label = field(TargetLabel | None, None),
    # The artifacts the linker is given for this entry, in link order: the
    # archive (or the objects of a virtual archive); the shared library, or the
    # interface the toolchain links against instead (`__shlib_intfs__/...`, a
    # `.tbd`); or the objects. Empty when the entry contributes no artifact (a
    # header-only library, a shared library with no sources, linked as nothing).
    #
    # An artifact with a content-based path has no path until it is built.
    # Shared-library interfaces always have one, archives and objects do on the
    # toolchains that enable it, so the JSON of a record with extras is written
    # only after every listed artifact is built, and fails with them; written
    # without extras, the record stays an analysis-time output. That is why
    # extras are opt-in.
    outputs = field(list[Artifact], []),
)

# These are targets or link groups that will be added to .linker.argsfile
# Targets will be expanded to .o files, link groups will be added to NEEDS
LinkGroupsDebugLinkableEntry = record(
    name = field(TargetLabel | Label | str),
    output_style = field(LibOutputStyle),
    # None unless xcode data is enabled.
    extras = field(LinkGroupsDebugLinkableEntryExtras | None, None),
)

# This is info about single output unit. It is either a final binary or
# one of link groups.
LinkGroupsDebugLinkableItem = record(
    ordered_linkables = field(list[LinkGroupsDebugLinkableEntry]),
)

LinkGroupsDebugLinkInfo = record(
    binary = field(LinkGroupsDebugLinkableItem),
    libs = field(dict[str, LinkGroupsDebugLinkableItem]),
)

LinkGroupInfo = provider(
    fields = {
        # Additional graphs needed to cover labels referenced by the groups above.
        # This is useful in cases where the consumer of this provider won't already
        # have deps covering these.
        # NOTE(agallagher): We do this to maintain existing behavior w/ the
        # standalone `link_group_map()` rule, but it's not clear if it's actually
        # desirable behavior.
        "graph": provider_field(typing.Any, default = None),  # LinkableGraph
        "groups": provider_field(dict[str, Group]),
        "groups_hash": provider_field(int),
        "mappings": provider_field(dict[Label, str]),
    },
)

LinkGroupDefinitions = provider(
    fields = {
        # The label is the top-level target
        "definitions": provider_field(typing.Callable[[Label, LinkStrategy], list[Group] | None]),
    },
)

_FILTER_ATTR = attrs.one_of(attrs.dep(providers = [GroupFilterInfo]), attrs.string())

def link_group_inlined_map_attr(root_attr):
    return attrs.list(
        attrs.tuple(
            # name
            attrs.string(),
            # list of mappings
            attrs.list(
                # a single mapping
                attrs.tuple(
                    # root node
                    attrs.one_of(root_attr, attrs.list(root_attr)),
                    # traversal
                    attrs.enum(Traversal.values()),
                    # filters, either `None`, a single filter, or a list of filters
                    # (which must all match).
                    attrs.option(attrs.one_of(attrs.list(_FILTER_ATTR), _FILTER_ATTR)),
                    # linkage
                    attrs.option(attrs.enum(Linkage.values())),
                ),
            ),
            # attributes
            attrs.option(
                attrs.dict(key = attrs.string(), value = attrs.one_of(attrs.source(), attrs.any()), sorted = False),
            ),
        ),
    )

LINK_GROUP_MAP_ATTR = attrs.option(
    attrs.one_of(
        attrs.dep(),  # LinkGroupInfo or LinkGroupInfoGenerator
        link_group_inlined_map_attr(
            # Inlined `link_group_map` will parse roots as `label`s, to avoid
            # bloating deps w/ unrelated mappings (e.g. it's common to use
            # a default mapping for all rules, which would otherwise add
            # unrelated deps to them).
            root_attr = attrs.option(attrs.label()),
        ),
    ),
    default = None,
)
