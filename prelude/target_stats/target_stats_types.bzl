# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Per-target codebase statistics, propagated across the dependency graph.

TargetStatsRecord = record(
    label = field(str),
    manifest = field(Artifact),
    manifest_with_inputs = field(typing.Any),
)

def _project_manifests(record: TargetStatsRecord) -> typing.Any:
    return record.manifest_with_inputs

TargetStatsInfoTSet = transitive_set(
    args_projections = {
        "manifests": _project_manifests,
    },
)

TargetStatsInfo = provider(
    fields = {
        "label": provider_field(str),
        "tset": provider_field(TargetStatsInfoTSet),
    },
)
