# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _remote_text_impl(ctx):
    text = ctx.attrs.text

    out = ctx.actions.declare_output("action_output", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["cp", text, out.as_output()]),
        category = "touch",
    )

    return [DefaultInfo(default_output = out)]

remote_text = rule(
    impl = _remote_text_impl,
    attrs = {
        "text": attrs.source(),
    },
)

def _symlink_dir_impl(ctx):
    remote_text = ctx.attrs.remote_text[DefaultInfo].default_outputs[0]
    link = ctx.actions.symlinked_dir(ctx.label.name, {"link": remote_text}, has_content_based_path = False)
    return [DefaultInfo(default_output = link)]

symlink_dir = rule(
    impl = _symlink_dir_impl,
    attrs = {
        "remote_text": attrs.dep(),
    },
)

def _check_impl(ctx):
    text = ctx.attrs.text
    symlink_dir = ctx.attrs.symlink_dir[DefaultInfo].default_outputs[0]

    out = ctx.actions.declare_output("out", has_content_based_path = False)

    ctx.actions.run(
        cmd_args(
            [
                "cp",
                cmd_args(symlink_dir, format = "{}/link"),
                out.as_output(),
            ],
            hidden = text,  # Invalidate this action when `text` changes.
        ),
        category = "test",
        local_only = True,
    )

    return [DefaultInfo(default_output = out)]

check = rule(
    impl = _check_impl,
    attrs = {
        "symlink_dir": attrs.dep(),
        "text": attrs.source(),
    },
)
