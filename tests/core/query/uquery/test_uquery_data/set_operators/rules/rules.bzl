# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

FooInfo = provider(
    fields = [
        "foo",
    ]
)

def _platform_impl(ctx):
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

foo_platform = rule(
    impl = _platform_impl,
    attrs = {},
)

def _impl(ctx):
    return [DefaultInfo(), FooInfo(foo = ctx.attrs.name + "_foo")]

_foo_library = rule(
    impl = _impl,
    attrs = {
        "deps": attrs.list(attrs.dep(), default = []),
        "srcs": attrs.list(attrs.source(), default = []),
    },
)

_default_platform = "root//platforms:platform1"

def foo_library(**kwargs):
    _foo_library(default_target_platform = _default_platform, **kwargs)
