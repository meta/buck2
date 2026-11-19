# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _remote_text_impl(ctx):
    out = ctx.actions.declare_output("action_output", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["cp", ctx.attrs.text, out.as_output()]),
        category = "cp",
    )
    return [DefaultInfo(default_output = out)]

remote_text = rule(
    impl = _remote_text_impl,
    attrs = {
        "text": attrs.source(),
    },
)

def _symlink_dir_impl(ctx):
    link = ctx.actions.symlinked_dir(
        ctx.label.name,
        {"link": _target(ctx)},
        has_content_based_path = False,
    )
    return [DefaultInfo(default_output = link)]

def _symlink_file_impl(ctx):
    link = ctx.actions.symlink_file(
        ctx.label.name,
        _target(ctx),
        has_content_based_path = False,
    )
    return [DefaultInfo(default_output = link)]

def _target(ctx):
    if ctx.attrs.dep != None:
        return ctx.attrs.dep[DefaultInfo].default_outputs[0]
    return ctx.attrs.src

_symlink_attrs = {
    "dep": attrs.option(attrs.dep(), default = None),
    "src": attrs.option(attrs.source(), default = None),
}

# A directory artifact holding a single symlink `link`, pointing either at
# another target's output or at a source file.
symlink_dir = rule(impl = _symlink_dir_impl, attrs = _symlink_attrs)

# The same target, but as a bare symlink artifact rather than a directory
# holding one. Its relative target escapes the artifact in both cases.
symlink_file = rule(impl = _symlink_file_impl, attrs = _symlink_attrs)

def _check_impl(ctx):
    dep = ctx.attrs.dep[DefaultInfo].default_outputs[0]
    if ctx.attrs.entry:
        input = cmd_args(dep, format = "{}/" + ctx.attrs.entry)
    else:
        input = dep

    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            ["cp", input, out.as_output()],
            # A symlink artifact's value is the path it points at, so it does
            # not change when the target's content does. Depending on the
            # target's own sources is what re-runs this action.
            hidden = ctx.attrs.hidden,
        ),
        category = "check",
        local_only = True,
    )
    return [DefaultInfo(default_output = out)]

# Copies what it reads through `dep` to its own output, so that a test can
# assert on what a local action saw.
check = rule(
    impl = _check_impl,
    attrs = {
        "dep": attrs.dep(),
        "entry": attrs.option(attrs.string(), default = None),
        "hidden": attrs.list(attrs.source(), default = []),
    },
)
