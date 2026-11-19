# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactTSet",  # @unused Used as a type
    "make_artifact_tset",
)

CxxTransitiveDiagnosticsInfo = provider(
    fields = {
        "transitive_diagnostics": provider_field(ArtifactTSet),
    },
)

def cxx_transitive_diagnostics_combine(ctx: AnalysisContext, diagnostics: list[Artifact], deps: list[Dependency]) -> CxxTransitiveDiagnosticsInfo:
    children = []
    for dep in deps:
        dep_diagnostics_info = dep.get(CxxTransitiveDiagnosticsInfo)
        if dep_diagnostics_info:
            children.append(dep_diagnostics_info.transitive_diagnostics)

    transitive_diagnostics = make_artifact_tset(
        actions = ctx.actions,
        label = ctx.label,
        artifacts = diagnostics,
        children = children,
    )

    return CxxTransitiveDiagnosticsInfo(
        transitive_diagnostics = transitive_diagnostics,
    )
