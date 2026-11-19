# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactTSet",
    "make_artifact_tset",
    "project_artifacts",
)

ModularizationDependencyGraphInfo = provider(
    fields = {
        "tset": provider_field(ArtifactTSet),
    },
)

def create_modularization_dep_graph_subtargets_and_provider(
    ctx: AnalysisContext,
    modularization_dependency_graph: Artifact | None,
    deps: list[Dependency],
) -> (dict[str, list[Provider]], ModularizationDependencyGraphInfo):
    children = [info.tset for info in filter(None, [dep.get(ModularizationDependencyGraphInfo) for dep in deps])]

    tset = make_artifact_tset(
        actions = ctx.actions,
        label = ctx.label if modularization_dependency_graph else None,
        artifacts = [modularization_dependency_graph] if modularization_dependency_graph else [],
        children = children,
    )

    all_graphs = project_artifacts(actions = ctx.actions, tsets = tset)

    sub_targets = {
        "all-modularization-dependency-graphs": [
            DefaultInfo(
                default_output = None,
                other_outputs = all_graphs,
            ),
        ],
        "modularization-dependency-graph": [
            DefaultInfo(
                default_output = modularization_dependency_graph,
                other_outputs = all_graphs,
            ),
        ],
    }

    info = ModularizationDependencyGraphInfo(tset = tset)

    return (sub_targets, info)
