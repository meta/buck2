# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _test_impl(ctx):
    tag = ctx.actions.artifact_tag()

    dep_file = ctx.actions.declare_output("depfile", has_content_based_path = False)
    app = ctx.actions.declare_output("app", has_content_based_path = False)

    seed = ctx.actions.write("seed", ctx.attrs.seed, has_content_based_path = False)

    ctx.actions.run(
        [
            "sh",
            "-c",
            'echo "../invalid" > "$1" && touch "$2" && echo "$3"',
            "--",
            tag.tag_artifacts(dep_file.as_output()),
            app.as_output(),
            seed,
        ],
        category = "test",
        dep_files = {"deps": tag},
    )

    return [
        DefaultInfo(
            default_output = app,
            sub_targets = {"dep_file": [DefaultInfo(default_output = dep_file)]},
        ),
    ]

test = rule(
    attrs = {
        "seed": attrs.string(default = ""),
    },
    impl = _test_impl,
)

def _n_outputs_tagged_as_dep_file_impl(ctx):
    app = ctx.actions.declare_output("app", has_content_based_path = False)
    args = cmd_args([
        "sh",
        "-c",
        "never_executed",
        "--",
        app.as_output(),
    ])

    tag = ctx.actions.artifact_tag()
    for i in range(ctx.attrs.num_dep_files):
        dep_file = ctx.actions.declare_output("depfile{}".format(i), has_content_based_path = False)
        args.add(tag.tag_artifacts(dep_file.as_output()))

    ctx.actions.run(
        args,
        category = "test",
        dep_files = {"deps": tag},
    )

    return [
        DefaultInfo(
            default_output = app,
        ),
    ]

n_outputs_tagged_as_dep_file = rule(
    attrs = {
        "num_dep_files": attrs.int(),
    },
    impl = _n_outputs_tagged_as_dep_file_impl,
)

def _same_tag_for_multiple_labels_impl(ctx):
    app = ctx.actions.declare_output("app", has_content_based_path = False)
    tag = ctx.actions.artifact_tag()
    dep_file = ctx.actions.declare_output("depfile", has_content_based_path = False)
    args = cmd_args([
        "sh",
        "-c",
        "never_executed",
        "--",
        app.as_output(),
        tag.tag_artifacts(dep_file.as_output()),
    ])

    ctx.actions.run(
        args,
        category = "test",
        dep_files = {"deps": tag, "deps2": tag},
    )

    return [
        DefaultInfo(
            default_output = app,
        ),
    ]

same_tag_for_multiple_labels = rule(
    attrs = {},
    impl = _same_tag_for_multiple_labels_impl,
)

def _input_tagged_multiple_times_impl(ctx):
    app = ctx.actions.declare_output("app", has_content_based_path = False)

    tag1 = ctx.actions.artifact_tag()
    tag2 = ctx.actions.artifact_tag()

    dep_file1 = ctx.actions.declare_output("depfile1", has_content_based_path = False)
    dep_file2 = ctx.actions.declare_output("depfile2", has_content_based_path = False)

    input = tag1.tag_artifacts(tag2.tag_artifacts(ctx.actions.write("input_tagged_multiple_times.txt", "input", has_content_based_path = False)))

    args = cmd_args([
        "sh",
        "-c",
        "never_executed",
        "--",
        app.as_output(),
        tag1.tag_artifacts(dep_file1.as_output()),
        tag2.tag_artifacts(dep_file2.as_output()),
        input,
    ])

    ctx.actions.run(
        args,
        category = "test",
        dep_files = {"deps1": tag1, "deps2": tag2},
    )

    return [
        DefaultInfo(
            default_output = app,
        ),
    ]

input_tagged_multiple_times = rule(
    attrs = {},
    impl = _input_tagged_multiple_times_impl,
)
