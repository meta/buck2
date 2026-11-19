# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

G = exec_group()

tool = rule(impl = lambda ctx: [DefaultInfo(), RunInfo(args = cmd_args("true"))], attrs = {})

r = rule(
    impl = lambda ctx: [DefaultInfo()],
    attrs = {"tools": attrs.option(attrs.list(attrs.exec_dep()), default = None)},
    exec_groups = [DefaultExecGroup, G],
)
