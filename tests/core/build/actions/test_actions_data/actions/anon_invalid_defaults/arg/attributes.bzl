# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# A test of various types of attribute

MirrorInfo = provider(fields = ["info"])

def _mirror_impl(ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo(), MirrorInfo(info = ctx.attrs)]

_mirror_arg = rule(
    impl = _mirror_impl,
    attrs = {
        "arg_attr": attrs.arg(default = "foo"),
    },
)

def _default_arg_fails(ctx: AnalysisContext) -> Promise:
    def f(_providers):
        return [DefaultInfo()]

    return ctx.actions.anon_target(_mirror_arg, {}).promise.map(f)

default_arg_fails = rule(impl = _default_arg_fails, attrs = {})

def _mirror_no_default_impl(ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo(), MirrorInfo(info = ctx.attrs)]

_mirror_no_default_arg = rule(
    impl = _mirror_no_default_impl,
    attrs = {
        "arg": attrs.arg(),
    },
)

_python = "import os; out = open(os.getenv('OUT'), 'wb'); out.write(os.urandom(50))"

def _arg_not_compatible_impl(ctx: AnalysisContext) -> Promise:
    def f(_providers):
        return [DefaultInfo()]

    return ctx.actions.anon_target(
        _mirror_no_default_arg,
        {
            "arg": ctx.attrs.arg,
        },
    ).promise.map(f)

arg_not_compatible = rule(
    impl = _arg_not_compatible_impl,
    attrs = {
        "arg": attrs.arg(default = _python),
    },
)
