# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _write(ctx):
    return [DefaultInfo(default_output = ctx.actions.write("a.txt", "a"))]

write = rule(impl = _write, attrs = {})

def _copy(ctx):
    src = ctx.attrs.src[DefaultInfo].default_outputs[0]
    return [DefaultInfo(default_output = ctx.actions.copy_file("b.txt", src))]

copy = rule(impl = _copy, attrs = {"src": attrs.dep()})

def _fail(ctx):
    out = ctx.actions.declare_output("never")
    ctx.actions.run(cmd_args(["sh", "-c", "exit 1", out.as_output()]), category = "fail", local_only = True)
    return [DefaultInfo(default_output = out)]

fail = rule(impl = _fail, attrs = {})

FilesInfo = provider(fields = ["files"])

Files = transitive_set(args_projections = {"paths": lambda value: value})

def _leaf(ctx):
    out = ctx.actions.write(ctx.label.name + ".txt", ctx.label.name)
    children = [dep[FilesInfo].files for dep in ctx.attrs.deps]
    return [
        DefaultInfo(default_output = out),
        FilesInfo(files = ctx.actions.tset(Files, value = out, children = children)),
    ]

leaf = rule(impl = _leaf, attrs = {"deps": attrs.list(attrs.dep(), default = [])})

def _consume(ctx):
    out = ctx.actions.declare_output("all.txt")
    files = ctx.attrs.dep[FilesInfo].files.project_as_args("paths")
    ctx.actions.run(
        cmd_args(["sh", "-c", 'cat "$@" > "$0"', out.as_output(), files]),
        category = "consume",
        local_only = True,
    )
    return [DefaultInfo(default_output = out)]

consume = rule(impl = _consume, attrs = {"dep": attrs.dep()})

def _dyn_f_impl(actions: AnalysisActions, src: ArtifactValue, out: OutputArtifact):
    actions.write(out, src.read_string())
    return []

_dyn_f = dynamic_actions(
    impl = _dyn_f_impl,
    attrs = {
        "out": dynattrs.output(),
        "src": dynattrs.artifact_value(),
    },
)

def _dyn(ctx):
    src = ctx.actions.write("dyn_input", "42")
    out = ctx.actions.declare_output("dyn_output")
    ctx.actions.dynamic_output_new(_dyn_f(src = src, out = out.as_output()))
    return [DefaultInfo(default_output = out)]

dyn = rule(impl = _dyn, attrs = {})
