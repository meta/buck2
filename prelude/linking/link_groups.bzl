# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//utils:dicts.bzl",
    "flatten_x",
)
load(
    ":link_info.bzl",
    "LinkInfos",
)
load(
    ":shared_libraries.bzl",
    "SharedLibraries",
)

# Information about a linkable node which explicitly sets `link_group`.
LinkGroupLib = record(
    # The label of the owning target (if any).
    label = field([Label, None], None),
    # The shared libs to package for this link group.
    shared_libs = field(SharedLibraries),
    # The link info to link against this link group.
    shared_link_infos = field(LinkInfos),
)

# Provider propagating info about transitive link group libs.
LinkGroupLibInfo = provider(
    fields = {
        # A map of link group names to their shared libraries.
        "libs": provider_field(typing.Any, default = None),  # dict[str, LinkGroupLib]
    }
)

EMPTY_LINK_GROUP_LIB_INFO = LinkGroupLibInfo(libs = {})

def gather_link_group_libs(
    libs: list[dict[str, LinkGroupLib]] = [], children: list[LinkGroupLibInfo] = [], deps: list[Dependency] = []
) -> dict[str, LinkGroupLib]:
    """
    Return all link groups libs deps and top-level libs.
    """
    return flatten_x(
        (libs + [c.libs for c in children] + [d[LinkGroupLibInfo].libs for d in deps if LinkGroupLibInfo in d]),
        fmt = 'conflicting link group roots for "{0}": {1} != {2}',
    )

def merge_link_group_lib_info(
    label: [Label, None] = None,
    name: [str, None] = None,
    shared_libs: [SharedLibraries, None] = None,
    shared_link_infos: [LinkInfos, None] = None,
    deps: list[Dependency] = [],
    children: list[LinkGroupLibInfo] = [],
) -> LinkGroupLibInfo:
    """
    Merge and return link group info libs from deps and the current rule wrapped
    in a provider.
    """
    libs = {}
    if name != None:
        libs[name] = LinkGroupLib(
            label = label,
            shared_libs = shared_libs,
            shared_link_infos = shared_link_infos,
        )
    merged = gather_link_group_libs(
        libs = [libs],
        deps = deps,
        children = children,
    )
    if not merged:
        return EMPTY_LINK_GROUP_LIB_INFO
    return LinkGroupLibInfo(
        libs = merged,
    )
