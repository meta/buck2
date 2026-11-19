# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def project(f: Artifact):
    return f

NameSet = transitive_set(
    args_projections = {
        "project": project,
    }
)

NameInfo = provider(fields = ["tset"])

def _test_impl(ctx):
    # Produce a file that contains our name.
    out = ctx.actions.write("out.txt", str(ctx.label.name) + "\n", has_content_based_path = False)

    # Produce a tset that is our file concatenated with all the files emitted by
    # our children.
    children = [d[NameInfo].tset for d in ctx.attrs.deps]
    tset = ctx.actions.tset(NameSet, value = out, children = children)

    # Concatenate all the files declared by the tset, into a single file
    # (agg.txt), which we'll return as our output.
    agg = ctx.actions.declare_output("agg.txt", has_content_based_path = False)
    ctx.actions.run(
        [
            "sh",
            "-c",
            'out="$1" && shift && cat "$@" > "$out"',
            "--",
            agg.as_output(),
            tset.project_as_args("project"),
        ],
        category = "test",
    )

    return [
        NameInfo(tset = tset),
        DefaultInfo(default_output = agg),
    ]

test = rule(
    impl = _test_impl,
    attrs = {
        "deps": attrs.list(attrs.dep(providers = [NameInfo]), default = []),
    },
)

def _test_duplication_impl(ctx):
    out = ctx.actions.write("out.txt", "hello world", has_content_based_path = False)

    tset1 = ctx.actions.tset(NameSet, value = out)
    tset2 = ctx.actions.tset(NameSet, value = out)

    combined_tset = ctx.actions.tset(NameSet, children = [tset1, tset2])

    combined_tset_traversal = list(combined_tset.traverse())

    if len(combined_tset_traversal) != 2:
        fail(
            "Expected traversal to be deduplicated by TSet identity, not value identity! Traversal is: {}".format(
                combined_tset_traversal,
            ),
        )

    return [
        DefaultInfo(default_output = out),
    ]

test_duplication = rule(
    impl = _test_duplication_impl,
    attrs = {
        "deps": attrs.list(attrs.dep(providers = [NameInfo]), default = []),
    },
)

OptionalArtifact = record(
    artifact = field(Artifact | None),
)

def project_as_args(optional_artifact: OptionalArtifact):
    if optional_artifact.artifact:
        return optional_artifact.artifact.short_path
    return None

OptionalArgsNameSet = transitive_set(
    args_projections = {
        "project": project_as_args,
    },
    json_projections = {
        "project_json": project_as_args,
    },
)

OptionalArgsNameInfo = provider(fields = ["tset"])

def _optional_args_impl(ctx):
    # Produce a file whose short-name is our name.
    if ctx.attrs.has_artifact:
        out = ctx.actions.write("{}".format(str(ctx.label.name)), "hello world", has_content_based_path = False)
    else:
        out = None

    # Produce a tset from our file and our children's files.
    children = [d[OptionalArgsNameInfo].tset for d in ctx.attrs.deps]
    tset = ctx.actions.tset(OptionalArgsNameSet, value = OptionalArtifact(artifact = out), children = children)

    # Write the projection of the tset to a file.
    agg = ctx.actions.declare_output("agg.txt", has_content_based_path = False)
    ctx.actions.write(agg, tset.project_as_args("project"))

    return [
        OptionalArgsNameInfo(tset = tset),
        DefaultInfo(default_output = agg),
    ]

optional_args = rule(
    impl = _optional_args_impl,
    attrs = {
        "deps": attrs.list(attrs.dep(providers = [OptionalArgsNameInfo]), default = []),
        "has_artifact": attrs.bool(default = True),
    },
)

OptionalJsonArgsNameSet = transitive_set(
    json_projections = {
        "project_json": project_as_args,
    },
)

OptionalJsonArgsNameInfo = provider(fields = ["tset"])

def _optional_json_args_impl(ctx):
    # Produce a file whose short-name is our name.
    if ctx.attrs.has_artifact:
        out = ctx.actions.write("{}".format(str(ctx.label.name)), "hello world", has_content_based_path = False)
    else:
        out = None

    # Produce a tset from our file and our children's files.
    children = [d[OptionalJsonArgsNameInfo].tset for d in ctx.attrs.deps]
    tset = ctx.actions.tset(OptionalJsonArgsNameSet, value = OptionalArtifact(artifact = out), children = children)

    # Concatenate all the files declared by the tset, into a single file
    # (agg.txt), which we'll return as our output.
    agg = ctx.actions.declare_output("agg.txt", has_content_based_path = False)

    ctx.actions.write_json(agg, tset.project_as_json("project_json"))

    return [
        OptionalJsonArgsNameInfo(tset = tset),
        DefaultInfo(default_output = agg),
    ]

optional_json_args = rule(
    impl = _optional_json_args_impl,
    attrs = {
        "deps": attrs.list(attrs.dep(providers = [OptionalJsonArgsNameInfo]), default = []),
        "has_artifact": attrs.bool(default = True),
    },
)
