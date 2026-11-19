# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

G = exec_group()

anon = rule(impl = lambda ctx: [DefaultInfo()], attrs = {})

def _impl(ctx):
    ctx.actions.anon_target(anon, {})
    return [DefaultInfo()]

r = rule(impl = _impl, attrs = {}, exec_groups = [G])
