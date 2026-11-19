# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _make_group():
    return exec_group()

r = rule(
    impl = lambda ctx: [DefaultInfo()],
    attrs = {},
    exec_groups = [_make_group()],
)
