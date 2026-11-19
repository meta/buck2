# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

HelloInfo = provider(fields = ["output"])

def _builder_impl(ctx: AnalysisContext) -> list[Provider]:
    hello = ctx.actions.write("hello.out", "hello", has_content_based_path = False)
    return [DefaultInfo(), HelloInfo(output = hello)]

_builder = anon_rule(
    impl = _builder_impl,
    attrs = {},
    artifact_promise_mappings = {
        "artifact": lambda x: x[HelloInfo].output,
    },
)

def _build_impl(ctx: AnalysisContext) -> list[Provider]:
    artifact = ctx.actions.anon_target(_builder, {}).artifact("artifact")
    ctx.actions.assert_short_path(artifact, short_path = "WRONG_PATH")
    return [DefaultInfo()]

build = rule(impl = _build_impl, attrs = {})

def _bad_short_path_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.declare_output("output", has_content_based_path = False)

    def f(ctx: AnalysisContext, _artifacts, outputs):
        ctx.actions.write(outputs[out], "")

    ctx.actions.dynamic_output(dynamic = [ctx.attrs.src], inputs = [], outputs = [out.as_output()], f = f)
    return [DefaultInfo(default_output = out)]

bad_short_path = rule(impl = _bad_short_path_impl, attrs = {"src": attrs.source()})
