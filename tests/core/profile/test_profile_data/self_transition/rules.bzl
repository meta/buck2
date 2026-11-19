# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# @nolint

def _zzz(ctx):
    _ignore = ctx
    return [DefaultInfo()]

def _transition_impl(platform, refs):
    _ignore = (platform, refs)
    return PlatformInfo(
        label = "<fgfgf>",
        configuration = ConfigurationInfo(
            constraints = {},
            values = {},
        ),
    )

_tr = transition(
    impl = _transition_impl,
    refs = {},
)

zzz = rule(
    impl = _zzz,
    attrs = {},
    cfg = _tr,
)
