# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

G = exec_group()

def _impl(_ctx):
    return [DefaultInfo()]

# No platform is both macOS and remote, so `G` has no compatible execution platform.
no_group_platform = rule(
    impl = _impl,
    attrs = {
        "_macos_tool": attrs.exec_dep(default = "//groups:macos_tool", exec_group = G),
        "_remote_tool": attrs.exec_dep(default = "//groups:remote_tool", exec_group = G),
    },
    exec_groups = [G],
)
