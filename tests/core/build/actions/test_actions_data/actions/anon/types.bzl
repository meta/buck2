# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _anon_impl(_ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo()]

_anon = rule(
    impl = _anon_impl,
    attrs = {
        "foo": attrs.string(),
    },
)

def _anon_target(ctx: AnalysisContext) -> AnonTarget:
    return ctx.actions.anon_target(
        _anon,
        {"foo": "barbaz"},
    )

def _anon_targets(ctx: AnalysisContext) -> AnonTargets:
    return ctx.actions.anon_targets([(_anon, {"foo": "bar"}), (_anon, {"foo": "baz"})])

# this doesn't do anything except make sure that the anon targets types are valid
def _types_impl(ctx: AnalysisContext) -> list[Provider]:
    _anon_target(ctx)
    _anon_targets(ctx)
    return [DefaultInfo()]

_types = rule(impl = _types_impl, attrs = {})

def types_test():
    _types(name = "types")
