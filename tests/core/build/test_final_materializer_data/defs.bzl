# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Rules for testing final materialization with tsets containing many artifacts.

The goal is to create targets with a configurable number of artifacts organized
in a transitive set structure, suitable for testing materialization
performance, especially in the no-op case where artifacts are already materialized.
"""

def _proj_identity(v):
    return v

ArtifactTSet = transitive_set(args_projections = {"identity": _proj_identity})

def _build_tset_tree(ctx, artifacts, nodes_per_tset):
    """
    Build a tset tree from a list of artifacts.

    Organizes artifacts into sub-tsets of size `nodes_per_tset`, then combines
    those sub-tsets into a balanced tree structure.
    """
    if len(artifacts) == 0:
        return None

    # Create leaf tsets for each artifact
    leaf_tsets = [ctx.actions.tset(ArtifactTSet, value = art) for art in artifacts]

    # Group into sub-tsets of size nodes_per_tset
    sub_tsets = []
    for i in range(0, len(leaf_tsets), nodes_per_tset):
        chunk = leaf_tsets[i : i + nodes_per_tset]
        if len(chunk) == 1:
            sub_tsets.append(chunk[0])
        else:
            sub_tsets.append(ctx.actions.tset(ArtifactTSet, children = chunk))

    # Combine sub-tsets into a balanced tree
    # Maximum depth needed is log2(max_tsets), use a fixed max of 20
    tsets = sub_tsets
    for _ in range(20):
        if len(tsets) <= 1:
            break
        next_level = []
        for i in range(0, len(tsets), 2):
            if i + 1 < len(tsets):
                combined = ctx.actions.tset(
                    ArtifactTSet,
                    children = [tsets[i], tsets[i + 1]],
                )
                next_level.append(combined)
            else:
                next_level.append(tsets[i])
        tsets = next_level

    return tsets[0] if tsets else None

def _artifact_generator_impl(ctx: AnalysisContext) -> list[Provider]:
    """
    A rule that generates artifacts and organizes them in a configurable tset structure.

    Configuration:
    - artifacts_per_node: Number of artifacts created per logical node
    - nodes_per_tset: Number of nodes grouped into each sub-tset
    - number_of_tsets: Number of sub-tsets to create

    Total artifacts = artifacts_per_node * nodes_per_tset * number_of_tsets
    """
    artifacts_per_node = ctx.attrs.artifacts_per_node
    nodes_per_tset = ctx.attrs.nodes_per_tset
    number_of_tsets = ctx.attrs.number_of_tsets

    total_artifacts = artifacts_per_node * nodes_per_tset * number_of_tsets
    artifacts = []

    # Create all unique artifacts
    for i in range(total_artifacts):
        art = ctx.actions.write(
            "artifact_{}.txt".format(i),
            "content_{}_{}".format(ctx.label.name, i),
            has_content_based_path = False,
        )
        artifacts.append(art)

    # Build a tset tree with the specified structure
    tset = _build_tset_tree(ctx, artifacts, nodes_per_tset * artifacts_per_node)

    # Create root output that depends on all artifacts via tset
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    if tset:
        ctx.actions.run(
            cmd_args(
                ["sh", "-c", 'echo "generated {} artifacts" > "$1"'.format(total_artifacts), "--", out.as_output()],
                hidden = [tset.project_as_args("identity")],
            ),
            category = "artifact_generator",
        )
        other_outputs = [tset.project_as_args("identity")]
    else:
        ctx.actions.write(out, "empty")
        other_outputs = []

    return [
        DefaultInfo(
            default_output = out,
            other_outputs = other_outputs,
        ),
    ]

artifact_generator = rule(
    impl = _artifact_generator_impl,
    attrs = {
        "artifacts_per_node": attrs.int(default = 1),
        "nodes_per_tset": attrs.int(default = 10),
        "number_of_tsets": attrs.int(default = 1),
    },
)
