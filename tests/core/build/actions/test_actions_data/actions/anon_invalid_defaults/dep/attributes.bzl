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

def _complex_source_impl(ctx: AnalysisContext) -> list[Provider]:
    artifact = ctx.actions.write("my_short_path", "", has_content_based_path = False)
    return [DefaultInfo(default_output = artifact)]

source = rule(impl = _complex_source_impl, attrs = {})

_dep_mirror = rule(
    impl = _mirror_impl,
    attrs = {
        "dep_attr": attrs.dep(default = "//anon_invalid_defaults/dep:dep"),
    },
)

def _complex_dep_impl(ctx: AnalysisContext) -> Promise:
    def f(_providers):
        return [DefaultInfo()]

    return ctx.actions.anon_target(_dep_mirror, {}).promise.map(f)

default_dep_fails = rule(impl = _complex_dep_impl, attrs = {})
