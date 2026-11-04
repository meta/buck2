# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""The libraries of an app compiled for Swift dynamic replacement, for hot reload.

An `apple_library` compiled for dynamic replacement contributes its label to a
transitive set; binaries and inner bundles forward it; a bundle writes the set
as its `[swift-dynamic-replacement-targets]` subtarget, a JSON list of target
labels. A hot reload session reads that list to know which libraries it can
make thunks for, whether the configuration or the library itself asked for
replacement. The module loads nothing from the bundle side: `apple_library`
loads it, and the bundle rules load `apple_library`.
"""

SWIFT_DYNAMIC_REPLACEMENT_TARGETS_SUBTARGET = "swift-dynamic-replacement-targets"

def _label(value: Label) -> str:
    return str(value.raw_target())

SwiftDynamicReplacementTargetsTSet = transitive_set(
    json_projections = {
        "labels": _label,
    },
)

SwiftDynamicReplacementTargetsInfo = provider(
    fields = {
        "targets": provider_field(SwiftDynamicReplacementTargetsTSet),
    },
)

def swift_dynamic_replacement_targets_info(ctx: AnalysisContext, deps: list[Dependency], replaceable: bool) -> [SwiftDynamicReplacementTargetsInfo, None]:
    """The provider for a target that is itself compiled for dynamic replacement
    when `replaceable`, and depends on `deps`; None when neither it nor anything
    below it is. A single child is passed on as is, so most of the graph
    allocates no new set."""
    children = [dep[SwiftDynamicReplacementTargetsInfo].targets for dep in deps if SwiftDynamicReplacementTargetsInfo in dep]
    if not replaceable and not children:
        return None
    if not replaceable and len(children) == 1:
        return SwiftDynamicReplacementTargetsInfo(targets = children[0])
    kwargs = {}
    if replaceable:
        kwargs["value"] = ctx.label
    if children:
        kwargs["children"] = children
    return SwiftDynamicReplacementTargetsInfo(targets = ctx.actions.tset(SwiftDynamicReplacementTargetsTSet, **kwargs))

def write_swift_dynamic_replacement_targets(ctx: AnalysisContext, info: [SwiftDynamicReplacementTargetsInfo, None]) -> Artifact:
    """The JSON list of the labels in `info`, empty without one. The same
    library can appear once per configuration it was built in, as under a
    universal binary."""
    output = ctx.actions.declare_output(SWIFT_DYNAMIC_REPLACEMENT_TARGETS_SUBTARGET + ".json")
    ctx.actions.write_json(
        output,
        info.targets.project_as_json("labels") if info else [],
        pretty = True,
    )
    return output
