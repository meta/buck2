# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _transition_to_reindeer_impl(platform, refs):
    _ignore = (platform, refs)  # buildifier: disable=unused-variable
    return PlatformInfo(label = "transitioned-to-reindeer", configuration = ConfigurationInfo(constraints = {}, values = {}))

transition_to_reindeer = transition(
    impl = _transition_to_reindeer_impl,
    refs = {},
)

def _simple_impl(ctx):
    return [DefaultInfo(default_output = ctx.actions.write("out", ["out"], has_content_based_path = False))]

simple = rule(
    impl = _simple_impl,
    attrs = {},
    # The configuration transition.
    cfg = transition_to_reindeer,
)
