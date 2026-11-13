# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _create_dummy_node(name, msg, compatible_with = ["ovr_config//:none"], visibility = None, **_kwargs):
    """A placeholder target that fails to build, for a target a build mode skips."""
    native.genrule(
        name = name,
        cmd = "echo 'Dummy node for queries. {}' >&2; false".format(msg),
        out = "unused",
        compatible_with = compatible_with,
        visibility = visibility,
    )

xplat_shims = struct(
    dummy_node = _create_dummy_node,
)
