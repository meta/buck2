# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

G = exec_group()

# The default group's exec dep does not exist, so diagnosing it fails while `G` has outcomes.
r = rule(
    impl = lambda ctx: [DefaultInfo()],
    attrs = {
        "_local_tool": attrs.exec_dep(default = "//groups:local_tool", exec_group = G),
        "_tool": attrs.exec_dep(default = "//errors/default_diagnosis_error:missing", exec_group = DefaultExecGroup),
    },
    exec_groups = [DefaultExecGroup, G],
)
