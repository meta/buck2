# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _echo_check_impl(ctx):
    app = ctx.actions.declare_output("app", has_content_based_path = False)

    if ctx.attrs.local_only == "true":
        local_only = True
    elif ctx.attrs.local_only == "false":
        local_only = False
    else:
        fail("Invalid local only: {}".format(ctx.attrs.local_only))

    ctx.actions.run(
        [
            "sh",
            "-c",
            'echo "$1" > "$2"',
            "--",
            ctx.attrs.param,
            app.as_output(),
        ],
        env = {
            "local_only": str(local_only),
        },
        category = ctx.attrs.category,
        local_only = local_only,
    )

    return [
        DefaultInfo(
            default_output = app,
        ),
    ]

echo_check = rule(
    attrs = {
        "category": attrs.string(),
        "local_only": attrs.string(),
        # NOTE: This does NOT get ignored.
        "param": attrs.string(),
        # NOTE: This gets ignored.
        "seed": attrs.string(),
    },
    impl = _echo_check_impl,
)

def _symlink_check_impl(ctx):
    data = ctx.actions.write("data", ctx.attrs.param, has_content_based_path = False)
    out = ctx.actions.declare_output("out/symlink", has_content_based_path = False)

    # NOTE: "data" and "out" will be next to each other here, hence ../data
    # NOTE: We use local_only since RE actually returns files for symlinks.
    ctx.actions.run(
        cmd_args(
            ["ln", "-s", "../data", out.as_output()],
            hidden = data,
        ),
        category = ctx.attrs.category,
        local_only = True,
    )

    return [
        DefaultInfo(default_output = out),
    ]

symlink_check = rule(
    attrs = {
        "category": attrs.string(),
        # NOTE: This gets ignored by the executed action (but not the data it links to)
        "param": attrs.string(),
    },
    impl = _symlink_check_impl,
)
