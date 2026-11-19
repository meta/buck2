# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

G = exec_group()

tool = rule(impl = lambda ctx: [DefaultInfo(), RunInfo(args = cmd_args("true"))], attrs = {})

anon = rule(
    impl = lambda ctx: [DefaultInfo()],
    attrs = {"cmd": attrs.arg(anon_target_compatible = True)},
)

def _impl(ctx):
    ctx.actions.anon_target(anon, {"cmd": ctx.attrs.cmd})
    return [DefaultInfo()]

r = rule(
    impl = _impl,
    attrs = {"cmd": attrs.arg(anon_target_compatible = True, exec_group = G)},
    exec_groups = [DefaultExecGroup, G],
)

grouped_anon = rule(
    impl = lambda ctx: [DefaultInfo()],
    attrs = {"cmd": attrs.arg(anon_target_compatible = True, exec_group = G)},
    exec_groups = [G],
)

def _grouped_anon_creator_impl(ctx):
    ctx.actions.anon_target(grouped_anon, {"cmd": "echo"})
    return [DefaultInfo()]

grouped_anon_creator = rule(impl = _grouped_anon_creator_impl, attrs = {})
