# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_library_utility.bzl", "cxx_attr_deps", "cxx_attr_exported_deps")
load("@prelude//utils:actions.bzl", "ActionExecutionAttributes")

LinkExecutionPreferenceTypes = [
    "any",
    "full_hybrid",
    "local",
    "local_only",
    "remote",
]

LinkExecutionPreference = enum(*LinkExecutionPreferenceTypes)

LinkExecutionPreferenceDeterminatorInfo = provider(
    fields = {
        # function that takes a list of target labels and the LinkExecutionPreferenceInfo of deps, and returns a LinkExecutionPreference
        "preference_for_links": provider_field(typing.Any, default = None),
    }
)

LinkExecutionPreferenceInfo = provider(
    fields = {
        "preference": provider_field(typing.Any, default = None),  # LinkExecutionPreference
    }
)

def link_execution_preference_attr():
    # The attribute is optional, allowing for None to represent that no preference has been set and we should fallback on the toolchain.
    return attrs.option(
        attrs.one_of(attrs.enum(LinkExecutionPreferenceTypes), attrs.dep(providers = [LinkExecutionPreferenceDeterminatorInfo])),
        default = None,
        doc = """
    The execution preference for linking. Options are:

    - any : No preference is set, and the link action will be performed based on buck2's executor configuration.
    - full_hybrid : The link action will execute both locally and remotely, regardless of buck2's executor configuration (if
        the executor is capable of hybrid execution). The use_limited_hybrid setting of the hybrid executor is ignored.
    - local : The link action will execute locally if compatible on current host platform.
    - local_only : The link action will execute locally, and error if the current platform is not compatible.
    - remote : The link action will execute remotely if a compatible remote platform exists, otherwise locally.

    The default is None, expressing that no preference has been set on the target itself.
    """,
    )

def get_link_execution_preference(ctx, links: list[Label]) -> LinkExecutionPreference:
    if not hasattr(ctx.attrs, "link_execution_preference"):
        fail("`get_link_execution_preference` called on a rule that does not support link_execution_preference!")

    link_execution_preference = ctx.attrs.link_execution_preference

    # If no preference has been set, we default to any.
    if not link_execution_preference:
        return LinkExecutionPreference("any")

    if not isinstance(link_execution_preference, Dependency):
        return LinkExecutionPreference(link_execution_preference)

    all_deps = cxx_attr_deps(ctx) + cxx_attr_exported_deps(ctx)
    deps_preferences = filter(None, [dep.get(LinkExecutionPreferenceInfo) for dep in all_deps])

    info = link_execution_preference[LinkExecutionPreferenceDeterminatorInfo]
    return info.preference_for_links(links, deps_preferences)

def get_action_execution_attributes(preference: LinkExecutionPreference) -> ActionExecutionAttributes:
    if preference == LinkExecutionPreference("any"):
        return ActionExecutionAttributes()
    elif preference == LinkExecutionPreference("full_hybrid"):
        return ActionExecutionAttributes(full_hybrid = True)
    elif preference == LinkExecutionPreference("local"):
        return ActionExecutionAttributes(prefer_local = True)
    elif preference == LinkExecutionPreference("local_only"):
        return ActionExecutionAttributes(local_only = True)
    elif preference == LinkExecutionPreference("remote"):
        return ActionExecutionAttributes(prefer_remote = True)
    else:
        fail("Unhandled LinkExecutionPreference: {}".format(str(preference)))
