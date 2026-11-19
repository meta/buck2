# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# This is how proper platform rule should be implemented.

def _proper_platform_impl(ctx):
    _unused = ctx  # buildifier: disable=unused-variable
    return [
        DefaultInfo(),
        PlatformInfo(
            label = str(ctx.label.raw_target()),
            configuration = ConfigurationInfo(
                constraints = {},
                values = {},
            ),
        ),
    ]

proper_platform = rule(
    impl = _proper_platform_impl,
    attrs = {},
)

# This rule returns a platform with wrong label.

def _wrong_platform_impl(ctx):
    _unused = ctx  # buildifier: disable=unused-variable
    return [
        DefaultInfo(),
        PlatformInfo(
            # Proper `platform` implementation should use `str(ctx.label.raw_target())`.
            label = "//:proper_platform",
            configuration = ConfigurationInfo(
                constraints = {},
                values = {},
            ),
        ),
    ]

wrong_platform = rule(
    impl = _wrong_platform_impl,
    attrs = {},
)

# Some rule

def _useless(ctx):
    _unused = ctx  # buildifier: disable=unused-variable
    return [
        DefaultInfo(),
    ]

useless = rule(
    impl = _useless,
    attrs = {},
)
