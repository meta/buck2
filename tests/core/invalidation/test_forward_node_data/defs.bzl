# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Rule with no attrs that produces an output. Useful if you want to be able to
# build literally anything
def _impl(ctx):
    return [DefaultInfo()]

def _transition_impl(platform, refs):
    _ignore = platform
    constraint = refs.value[ConstraintValueInfo]
    return PlatformInfo(
        label = "<fgfgf>",
        configuration = ConfigurationInfo(
            constraints = {
                constraint.setting.label: constraint,
            },
            values = {},
        ),
    )

_tr = transition(
    impl = _transition_impl,
    refs = {"value": "//:value"},
)

self_transitioned_build = rule(
    impl = _impl,
    cfg = _tr,
    attrs = {},
)

def _alias_impl(ctx):
    return ctx.attrs.actual.providers

alias = rule(
    impl = _alias_impl,
    attrs = {
        "actual": attrs.dep(),
    },
)
