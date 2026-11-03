# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Capabilities registration provider and tset for build graph propagation.

CapabilitiesRegistrationInfo is a provider that carries capability metadata
through the dependency graph via a transitive set (tset). Each library with
capability declarations creates a tset entry and merges it with its deps'
tsets. At binary build time, a manifest rule projects the final tset into
a support manifest JSON.
"""

def _project_as_manifest_json(entry):
    """Project a CapabilitiesRegistrationEntry to its manifest JSON representation."""
    result = {
        "kind": entry.kind,
        "style_id": entry.style_id,
    }
    if entry.props:
        result["props"] = entry.props
    if entry.features:
        result["features"] = entry.features
    if entry.exception_metadata:
        result["exception_metadata"] = entry.exception_metadata
    return result

CapabilitiesRegistrationEntry = record(
    style_id = field(str),
    kind = field(str),
    props = field(dict[str, typing.Any], {}),
    features = field(dict[str, typing.Any], {}),
    exception_metadata = field(dict[str, str], {}),
)

CapabilitiesRegistrationTSet = transitive_set(
    json_projections = {
        "manifest_entries": _project_as_manifest_json,
    },
)

CapabilitiesRegistrationInfo = provider(
    fields = {
        "tset": provider_field(CapabilitiesRegistrationTSet | None, default = None),
    }
)

def capabilities_registration_providers(ctx) -> list:
    """Collect CapabilitiesRegistrationInfo from deps and create tset entry if this target has registrations."""
    children = []
    for dep in ctx.attrs.deps + ctx.attrs.exported_deps:
        info = dep.get(CapabilitiesRegistrationInfo)
        if info and info.tset:
            children.append(info.tset)

    registrations = getattr(ctx.attrs, "capabilities_registrations", None)
    entries = []
    if registrations:
        for reg in registrations:
            entries.append(
                CapabilitiesRegistrationEntry(
                    style_id = reg["style_id"],
                    kind = reg["kind"],
                    props = reg.get("props", {}),
                    features = reg.get("features", {}),
                    exception_metadata = reg.get("exception_metadata", {}),
                )
            )

    if not entries and not children:
        return []

    if entries:
        for entry in entries:
            tset = ctx.actions.tset(CapabilitiesRegistrationTSet, value = entry, children = children)
            children = [tset]
        tset = children[0]
    else:
        tset = ctx.actions.tset(CapabilitiesRegistrationTSet, children = children)

    return [CapabilitiesRegistrationInfo(tset = tset)]
