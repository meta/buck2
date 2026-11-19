# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

G = exec_group()

tool = rule(impl = lambda ctx: [DefaultInfo(), RunInfo(args = cmd_args("true"))], attrs = {})

# Each attribute mixes args in `G` or `DefaultExecGroup` with args without `exec_group`; only
# the latter reject tools.
r = rule(
    impl = lambda ctx: [DefaultInfo()],
    attrs = {
        "default_group": attrs.tuple(
            attrs.arg(exec_group = DefaultExecGroup),
            attrs.list(attrs.arg()),
            default = ("$(exe //errors/mixed_args:tool)", ["-g"]),
        ),
        "either": attrs.option(attrs.one_of(attrs.arg(exec_group = G), attrs.list(attrs.arg())), default = None),
        "mapping": attrs.dict(attrs.arg(), attrs.arg(exec_group = G), default = {}),
        "pair": attrs.option(attrs.tuple(attrs.arg(exec_group = G), attrs.list(attrs.arg())), default = None),
        "with_default": attrs.tuple(
            attrs.arg(exec_group = G),
            attrs.list(attrs.arg()),
            default = ("$(exe //errors/mixed_args:tool)", ["-g"]),
        ),
    },
    exec_groups = [DefaultExecGroup, G],
)
