# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

MirrorInfo = provider(fields = ["info"])

ExecDepInfo = provider(fields = ["info"])

def _passthrough_impl(ctx):
    return [DefaultInfo(), ExecDepInfo(info = ctx.attrs.exec_dep)]

passthrough = rule(
    impl = _passthrough_impl,
    attrs = {
        "exec_dep": attrs.exec_dep(),
    },
)

def _assert_eq(a, b):
    if a != b:
        fail("Expected {} == {}".format(a, b))

def _mirror_impl(ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo(), MirrorInfo(info = ctx.attrs)]

_mirror_exec_dep = rule(
    impl = _mirror_impl,
    attrs = {
        "exec_dep": attrs.exec_dep(),
    },
)

def _exec_dep_good_impl(ctx: AnalysisContext) -> Promise:
    def f(providers):
        res = providers[MirrorInfo].info
        _assert_eq(res.exec_dep.label.configured_target().name, "remote_only")
        return [DefaultInfo()]

    return ctx.actions.anon_target(
        _mirror_exec_dep,
        {
            "exec_dep": ctx.attrs.dep[ExecDepInfo].info,
        },
    ).promise.map(f)

exec_dep_good = rule(
    impl = _exec_dep_good_impl,
    attrs = {
        "dep": attrs.dep(),
    },
)

def _exec_dep_bad_impl(ctx: AnalysisContext) -> Promise:
    def f(providers):
        res = providers[MirrorInfo].info
        _assert_eq(res.exec_dep, "remote_only")
        return [DefaultInfo()]

    return ctx.actions.anon_target(
        _mirror_exec_dep,
        {
            "exec_dep": ctx.attrs.dep[ExecDepInfo].info,
        },
    ).promise.map(f)

exec_dep_bad = rule(
    impl = _exec_dep_bad_impl,
    attrs = {
        "dep": attrs.dep(),
    },
)

def _exec_dep_rejects_dep_impl(ctx: AnalysisContext) -> Promise:
    def f(_providers):
        return [DefaultInfo()]

    return ctx.actions.anon_target(
        _mirror_exec_dep,
        {
            "exec_dep": ctx.attrs.dep,
        },
    ).promise.map(f)

exec_dep_rejects_dep = rule(
    impl = _exec_dep_rejects_dep_impl,
    attrs = {
        "dep": attrs.dep(),
    },
)
