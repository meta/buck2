# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//linking:types.bzl", "Linkage")

# Label for special group mapping which makes every target associated with it to be included in all groups
MATCH_ALL_LABEL = "MATCH_ALL"

# Label for special group mapping which makes every target associated with it to be linked directly
# against the final binary
NO_MATCH_LABEL = "NO_MATCH"

# Label for special group mapping which causes every target associated with it to be included
# in any groups that contain any targets with direct dependencies on it.
# Sample use case: Vendor-supplied shared libraries that are used from multiple link-groups.
MATCH_DIRECT_DEPS_LABEL = "MATCH_DIRECT_DEPS"

Traversal = enum(
    # Includes the target and all of it's transitive dependencies in the group.
    "tree",
    # Includes only the target in the group.
    "node",
    # Uses pattern and separates all targets by full folder path.
    "subfolders",
    # Includes targets found in the transitive deps of *any* roots.
    # Filters for these mappings will be applied to the intersected deps.
    "intersect_any_roots",
)

GroupFilterInfo = provider(
    fields = {
        # What should be dumped in the link-groups-info subtarget.
        "info": provider_field(dict[str, typing.Any]),
        # A function which is given an optional root label, a target label and
        # a list of its labels returns if the target label matches.
        "matches": provider_field(typing.Callable[[[Label, None], Label, list[str]], bool]),
        # If the matching should stop once the filter no longer matches.
        "stop_at_first_non_match": provider_field(bool, default = False),
    },
)

# Representation of a parsed group mapping
GroupMapping = record(
    # The root to apply this mapping to.
    roots = field(list[Label], []),
    # The type of traversal to use.
    traversal = field(Traversal, Traversal("tree")),
    # Optional filter type to apply to the traversal.
    filters = field(list[GroupFilterInfo], []),
    # Preferred linkage for this target when added to a link group.
    preferred_linkage = field([Linkage, None], None),
)

# Representation of group attributes
GroupAttrs = record(
    # Use distributed thinlto to build the link group shared library.
    enable_distributed_thinlto = field(bool, False),
    # Enable this link group if the binary's node count exceeds the given threshold
    enable_if_node_count_exceeds = field([int, None], None),
    # Discard all dependencies in the link group, useful for dropping unused dependencies
    # from the build graph.
    discard_group = field(bool, False),
    # Adds additional linker flags used to link the link group shared object.
    linker_flags = field(list[typing.Any], []),
    # Passes a linker script to the link group shared library's link command.
    linker_script = field([Artifact, None], None),
    # Passes a version script to the link group shared library's link command.
    # Tagging a group's symbols with a version keeps them from satisfying an
    # identically-named *versioned* reference in another copy of the same library
    # loaded into the process, which is otherwise allowed: glibc lets an
    # unversioned definition satisfy a versioned reference, so whichever copy
    # reaches the global scope first captures the other.
    version_script = field([Artifact, None], None),
    # Adds additional linker flags to apply to dependents that link against the
    # link group's shared object.
    exported_linker_flags = field(list[typing.Any], []),
    # Controls link execution location: "any" (default), "full_hybrid", "local",
    # "local_only", or "remote".
    link_execution_preference = field([str, None, typing.Any], None),
    # Wraps the link group shared library with `--no-as-needed/--as-needed`,
    # used for link groups that are required at runtime but not statically referenced.
    # Only applicable to gnu.
    no_as_needed = field(bool, False),
    # Requires root nodes in specs to always exist in dependency graph.
    # Otherwise fails.
    requires_root_node_exists = field(bool, True),
    # For certain wide-scale generic link groups we want to enable
    # initial duplicate analysis. This is useful for detecting dduplicated symbols problem early
    # for automatoc link groups that we not aware about (e.g. evicting whole root package folder into link group)
    prohibit_file_duplicates = field(bool, False),
    # Uses optimized compilation outputs if available.
    prefer_optimized_experimental = field(bool, False),
)

# Types of group traversal
GroupDefinition = enum(
    # Group is explicitly defined in mapping provided by user.
    # That is the default behavior.
    "explicit",
    # Group is implicitly created during mapping computations.
    # For example, group can be created for "subfolders" traversal.
    "implicit",
)

# Representation of a parsed group
Group = record(
    # The name for this group.
    name = str,
    # The mappings that are part of this group.
    mappings = list[GroupMapping],
    attrs = GroupAttrs,
    definition_type = field(GroupDefinition, GroupDefinition("explicit")),
)

def should_discard_group(group: [Group, None]) -> bool:
    return group != None and group.attrs.discard_group
