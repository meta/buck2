# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

G = exec_group()

tool = rule(impl = lambda ctx: [DefaultInfo(), RunInfo(args = cmd_args("true"))], attrs = {})

# The default puts a tool in the tuple position without `exec_group`.
r = rule(
    impl = lambda ctx: [DefaultInfo()],
    attrs = {
        "with_default": attrs.tuple(
            attrs.arg(exec_group = G),
            attrs.list(attrs.arg()),
            default = ("-O2", ["$(exe //errors/mixed_args_default:tool)"]),
        ),
    },
    exec_groups = [DefaultExecGroup, G],
)
