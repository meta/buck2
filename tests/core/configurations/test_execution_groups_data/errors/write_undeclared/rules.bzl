# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

Declared = exec_group()
Other = exec_group()

def _impl(ctx):
    out = ctx.actions.write("args.txt", "", exec_group = Other)
    return [DefaultInfo(default_output = out)]

r = rule(impl = _impl, attrs = {}, exec_groups = [Declared])
