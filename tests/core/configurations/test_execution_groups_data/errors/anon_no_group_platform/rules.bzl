# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

G = exec_group()

def _anon_impl(ctx):
    out = ctx.actions.declare_output("out")
    ctx.actions.run(["true", out.as_output()], category = "anon")
    return [DefaultInfo(default_output = out)]

_anon = anon_rule(
    impl = _anon_impl,
    attrs = {},
    artifact_promise_mappings = {"out": lambda p: p[DefaultInfo].default_outputs[0]},
)

def _impl(ctx):
    if ctx.attrs.plural:
        anon = ctx.actions.anon_targets([(_anon, {})], exec_group = G).anon_targets[0]
    else:
        anon = ctx.actions.anon_target(_anon, {}, exec_group = G)
    return [DefaultInfo(default_output = anon.artifact("out"))]

# No platform is both macOS and remote, so `G` has no compatible execution platform.
r = rule(
    impl = _impl,
    attrs = {
        "g_constraints": attrs.exec_compatible_with(
            exec_group = G,
            default = ["//config:macos", "//config:remote"],
        ),
        "plural": attrs.bool(default = False),
    },
    exec_groups = [G],
)
