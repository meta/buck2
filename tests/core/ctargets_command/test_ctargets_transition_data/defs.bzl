# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _clay_transition_impl(platform, refs):
    _ignore = (platform, refs)  # buildifier: disable=unused-variable
    return PlatformInfo(label = "<clay>", configuration = ConfigurationInfo(constraints = {}, values = {}))

clay_transition = transition(
    impl = _clay_transition_impl,
    refs = {},
)

def _clay_library_impl(ctx):
    _ignore = ctx  # buildifier: disable=unused-variable
    return [DefaultInfo()]

clay_library = rule(
    impl = _clay_library_impl,
    attrs = {"value": attrs.string()},
    cfg = clay_transition,
)
