# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# A very basic test that two things can share a single anon target

def _assert_eq(a, b):
    if a != b:
        fail("Expected {} == {}".format(a, b))

def _builder_impl(ctx: AnalysisContext) -> list[Provider]:
    a = ctx.actions.write("input", "hello", has_content_based_path = False)
    b = ctx.actions.declare_output("output", has_content_based_path = False)
    ctx.actions.run(cmd_args("cp", a, b.as_output()), category = "cp")
    return [DefaultInfo(default_output = b)]

_builder = rule(impl = _builder_impl, attrs = {})

def _build_impl(ctx: AnalysisContext) -> Promise:
    return ctx.actions.anon_target(_builder, {}).promise.map(lambda x: [x[DefaultInfo]])

_build = rule(impl = _build_impl, attrs = {})

def _check_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.declare_output("output", has_content_based_path = False)

    def f(ctx: AnalysisContext, artifacts, outputs):
        _assert_eq(artifacts[ctx.attrs.dep].read_string(), "hello")
        ctx.actions.write(outputs[out], "")

    ctx.actions.dynamic_output(dynamic = [ctx.attrs.dep], inputs = [], outputs = [out.as_output()], f = f)
    return [DefaultInfo(default_output = out)]

_check = rule(impl = _check_impl, attrs = {"dep": attrs.source()})

def build_test():
    _build(name = "build")
    _check(name = "build_check", dep = ":build")
