# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

G = exec_group()

# The write names no group, so it would use the undeclared default group; the one in `G` is fine.
def _impl(ctx):
    in_group = ctx.actions.write("in_group.txt", "", exec_group = G)
    out = ctx.actions.write("out.txt", "")
    return [DefaultInfo(default_outputs = [in_group, out])]

r = rule(impl = _impl, attrs = {}, exec_groups = [G])
