# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

G = exec_group()

r = rule(
    impl = lambda ctx: [DefaultInfo()],
    attrs = {"_tool": attrs.exec_dep(default = "//groups:any_tool", exec_group = G)},
)
