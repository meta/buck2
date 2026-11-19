# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _my_transition_impl(platform, refs):
    _ignore = (platform, refs)  # buildifier: disable=unused-variable
    return {
        "astrologer": PlatformInfo(label = "<astrologer>", configuration = ConfigurationInfo(constraints = {}, values = {})),
        "vagabond": PlatformInfo(label = "<vagabond>", configuration = ConfigurationInfo(constraints = {}, values = {})),
    }

my_transition = transition(
    impl = _my_transition_impl,
    refs = {},
    split = True,
)

def _universe_impl(ctx):
    _ignore = ctx  # buildifier: disable=unused-variable

    # Do not build anything, just configure dependencies.
    return [DefaultInfo()]

universe = rule(
    impl = _universe_impl,
    attrs = {
        "split_dep": attrs.split_transition_dep(cfg = my_transition),
    },
)

def _simple_impl(ctx):
    out = ctx.actions.write("out", cmd_args(str(ctx.label), format = "$$${}$$$"), has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

simple = rule(
    impl = _simple_impl,
    attrs = {},
)
