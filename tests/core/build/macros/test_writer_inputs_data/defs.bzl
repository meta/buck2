# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _failing_output(ctx, name):
    output = ctx.actions.declare_output(name, has_content_based_path = True)
    ctx.actions.run(
        cmd_args("fbpython", "-c", "raise RuntimeError('Execution resource requested')", hidden = output.as_output()),
        category = "fail_resource",
        identifier = name,
        local_only = True,
    )
    return output

def _input_impl(ctx):
    name = ctx.label.name
    primary = _failing_output(ctx, name + ".primary") if ctx.attrs.fail else ctx.actions.write(name + ".primary", name, has_content_based_path = True)
    if not ctx.attrs.resources:
        return [DefaultInfo(default_output = primary)]
    associated = _failing_output(ctx, name + ".associated")
    other = _failing_output(ctx, name + ".other")
    return [
        DefaultInfo(
            default_output = primary.with_associated_artifacts([associated]),
            other_outputs = [other],
            sub_targets = {"primary": [DefaultInfo(default_output = primary)]},
        )
    ]

input = rule(
    impl = _input_impl,
    attrs = {
        "fail": attrs.bool(default = False),
        "resources": attrs.bool(default = True),
    },
)

def _writer_impl(ctx):
    content = cmd_args(ctx.attrs.inline, ctx.attrs.macro, ctx.attrs.direct[DefaultInfo].default_outputs, hidden = ctx.attrs.hidden)
    if ctx.attrs.origin:
        content = cmd_args(content, relative_to = (ctx.attrs.origin[DefaultInfo].default_outputs[0], 1))
    content = ctx.actions.artifact_tag().tag_artifacts(content)
    script, macros = ctx.actions.write("script", content, allow_args = True, with_inputs = ctx.attrs.with_inputs, has_content_based_path = True)
    json_output = ctx.actions.declare_output("inputs.json", has_content_based_path = True)
    json = ctx.actions.write_json(
        json_output,
        cmd_args(ctx.attrs.inline, ctx.attrs.direct[DefaultInfo].default_outputs, hidden = ctx.attrs.hidden),
        with_inputs = ctx.attrs.with_inputs,
    )
    run = ctx.actions.declare_output("run")
    ctx.actions.run(
        cmd_args(
            "fbpython",
            "-c",
            "import pathlib, sys; pathlib.Path(sys.argv[1]).write_text('ran')",
            run.as_output(),
            hidden = [script, content],
        ),
        category = "execute",
        local_only = True,
    )
    return [
        DefaultInfo(
            default_output = script,
            sub_targets = {
                "json": [DefaultInfo(default_output = json_output, other_outputs = [json])],
                "macros": [DefaultInfo(default_outputs = macros)],
                "run": [DefaultInfo(default_output = run)],
            },
        )
    ]

writer = rule(
    impl = _writer_impl,
    attrs = {
        "direct": attrs.dep(),
        "hidden": attrs.arg(),
        "inline": attrs.arg(),
        "macro": attrs.arg(),
        "origin": attrs.option(attrs.dep(), default = None),
        "with_inputs": attrs.bool(default = False),
    },
)
