# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

ArgInfo = provider(fields = ["cmd"])

def _writer_impl(ctx):
    out = ctx.actions.declare_output("cmd.txt", has_content_based_path = False)
    ctx.actions.write(out, ctx.attrs.cmd, allow_args = True)
    return [DefaultInfo(default_output = out)]

_writer = anon_rule(
    impl = _writer_impl,
    attrs = {"cmd": attrs.arg(anon_target_compatible = True)},
    artifact_promise_mappings = {"out": lambda p: p[DefaultInfo].default_outputs[0]},
)

# Passes `cmd`, or the `cmd` of `provider`, to an anon target. `extra` exec deps only change this
# target's execution platform resolution.
def _creator_impl(ctx):
    cmd = ctx.attrs.provider[ArgInfo].cmd if ctx.attrs.provider else ctx.attrs.cmd
    anon = ctx.actions.anon_target(_writer, {"cmd": cmd})
    return [DefaultInfo(default_output = anon.artifact("out"))]

creator = rule(
    impl = _creator_impl,
    attrs = {
        "cmd": attrs.option(attrs.arg(anon_target_compatible = True), default = None),
        "extra": attrs.list(attrs.exec_dep(), default = []),
        "provider": attrs.option(attrs.dep(providers = [ArgInfo]), default = None),
    },
)

arg_provider = rule(
    impl = lambda ctx: [DefaultInfo(), ArgInfo(cmd = ctx.attrs.cmd)],
    attrs = {"cmd": attrs.arg(anon_target_compatible = True)},
)
