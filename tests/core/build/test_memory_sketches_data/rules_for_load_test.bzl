# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Allocate ~1MB at load time so we can verify load peak sketches
# capture load-phase memory.
LOAD_DATA = list(range(1000000 // 8))

def _simple_load_rule_impl(ctx):
    out = ctx.actions.write("out.txt", "")
    return [DefaultInfo(default_output = out)]

simple_load_rule = rule(
    impl = _simple_load_rule_impl,
    attrs = {},
)
