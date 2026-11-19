# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:type_defs.bzl", "is_list")
load(
    "@prelude//utils:utils.bzl",
    "flatten",
)

# Generic tag to provide more information about the artifact
ArtifactInfoTag = enum(
    # Describes artifacts required for debugging Swift code, including
    # swiftmodule files, swiftinterface files, clang modules and modulemaps.
    "swift_debug_info",
)

ArtifactInfo = record(
    label = field(Label),
    artifacts = field(list[Artifact]),
    tags = field(list[ArtifactInfoTag]),
)

def stringify_artifact_label(value: Label | str) -> str:
    if type(value) == "string":
        return value
    return str(value.raw_target())

def _get_artifacts(entries: list[ArtifactInfo]) -> list[Artifact]:
    return flatten([entry.artifacts for entry in entries])

_ArtifactTSet = transitive_set(
    args_projections = {
        "artifacts": _get_artifacts,
    },
)

ArtifactTSet = record(
    _tset = field([_ArtifactTSet, None], None),
)

# Simple empty `ArtifactTSet` that is shared to reduce memory usage.
EmptyArtifactTSet = ArtifactTSet()

def make_artifact_tset(
    actions: AnalysisActions,
    # Must be non-`None` if artifacts are passed in to `artifacts`.
    label: Label | None = None,
    artifacts: list[Artifact] = [],
    infos: list[ArtifactInfo] = [],
    children: list[ArtifactTSet] = [],
    tags: list[ArtifactInfoTag] = [],
) -> ArtifactTSet:
    expect(
        label != None or not artifacts,
        "must pass in `label` to associate with artifacts",
    )

    single_child = None  # type: ArtifactTSet | None
    children_orig = children
    children = []  # type: list[_ArtifactTSet]
    for c in children_orig:
        # As a convenience for our callers, filter our `None` children.
        if c._tset != None:
            children.append(c._tset)
            single_child = c

    has_values = artifacts or infos
    if not has_values and not children:
        return EmptyArtifactTSet

    if not has_values and len(children) == 1:
        # If we have a single child, just return it
        # rather than allocating and retaining additional memory.
        return single_child

    # Build list of all non-child values.
    values = []
    if artifacts:
        values.append(ArtifactInfo(label = label, artifacts = artifacts, tags = tags))
    values.extend(infos)

    # We only build a `_ArtifactTSet` if there's something to package.
    kwargs = {}
    if values:
        kwargs["value"] = values
    if children:
        kwargs["children"] = children
    return ArtifactTSet(
        _tset = actions.tset(_ArtifactTSet, **kwargs),
    )

def project_artifacts(actions: AnalysisActions, tsets: ArtifactTSet | list[ArtifactTSet] = []) -> list[TransitiveSetArgsProjection]:
    """
    Helper to project a list of optional tsets.
    """

    if is_list(tsets):
        tset = make_artifact_tset(
            actions = actions,
            children = tsets,
        )
    else:
        tset = tsets

    if tset._tset == None:
        return []

    return [tset._tset.project_as_args("artifacts")]
