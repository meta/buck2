# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _write_impl(ctx):
    out = ctx.actions.write("out.txt", ctx.attrs.content, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

write = rule(
    impl = _write_impl,
    attrs = {
        "content": attrs.string(),
    },
)

def _produce_impl(ctx):
    out = ctx.actions.declare_output("produced.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["fbpython", ctx.attrs.script, "--out", out.as_output()]),
        category = "produce",
    )
    return [DefaultInfo(default_output = out)]

# A local action's output, with whatever mode the tool left it.
produce = rule(
    impl = _produce_impl,
    attrs = {
        "script": attrs.source(),
    },
)

def _copy_source_impl(ctx):
    out = ctx.actions.copy_file("copied.txt", ctx.attrs.src)
    return [DefaultInfo(default_output = out)]

# A source file copied in the daemon.
copy_source = rule(
    impl = _copy_source_impl,
    attrs = {
        "src": attrs.source(),
    },
)

def _incremental_impl(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            ["fbpython", ctx.attrs.script, "--out", out.as_output()],
            hidden = [ctx.actions.write("invalidate", ctx.attrs.invalidate, has_content_based_path = False)],
        ),
        category = "incremental",
        no_outputs_cleanup = True,
        metadata_env_var = "METADATA_PATH",
        metadata_path = "metadata.json",
    )
    return [
        DefaultInfo(out),
        RunInfo(args = ["cat", out]),
    ]

# An incremental action: its previous output stays in place for the next run, which appends to it.
incremental = rule(
    impl = _incremental_impl,
    attrs = {
        "invalidate": attrs.string(),
        "script": attrs.source(),
    },
)

def _overlay_dir_impl(ctx):
    base = ctx.actions.copied_dir(
        "base",
        {
            "kept.txt": ctx.actions.write("kept.txt", "kept", has_content_based_path = False),
            "replaced.txt": ctx.actions.write("old.txt", "old", has_content_based_path = False),
        },
        has_content_based_path = False,
    )
    out = ctx.actions.copied_dir(
        "overlay",
        {
            "": base,
            "replaced.txt": ctx.actions.write("new.txt", "new", has_content_based_path = False),
        },
        has_content_based_path = False,
    )
    return [DefaultInfo(default_output = out)]

# A copied dir whose later entry replaces a file an earlier entry lays down.
overlay_dir = rule(
    impl = _overlay_dir_impl,
    attrs = {},
)
