# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# A very basic test that two things can share a single anon target

SubtargetInfo = provider(fields = ["information"])

def _assert_eq(a, b):
    if a != b:
        fail("Expected {} == {}".format(a, b))

def _anon_impl(ctx: AnalysisContext) -> list[Provider]:
    output = ctx.actions.write("hello.txt", "hello", has_content_based_path = False)
    return [DefaultInfo(default_outputs = [output])]

_anon = rule(impl = _anon_impl, attrs = {"dep": attrs.dep()})

def _subtarget_impl(ctx: AnalysisContext) -> Promise:
    def f(xs):
        _assert_eq(xs[0][DefaultInfo].default_outputs, xs[1][DefaultInfo].default_outputs)
        return [DefaultInfo()]

    child = ctx.attrs.child
    base_child = ctx.attrs.base.sub_target("child")
    _assert_eq(base_child[SubtargetInfo].information, child[SubtargetInfo].information)
    _assert_eq(base_child.label, child.label)
    return ctx.actions.anon_targets([(_anon, {"dep": child}), (_anon, {"dep": base_child})]).promise.map(f)

_subtarget = rule(
    impl = _subtarget_impl,
    attrs = {
        "base": attrs.dep(),
        "child": attrs.dep(),
    },
)

def _base_impl(_ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo(sub_targets = {"child": [DefaultInfo(), SubtargetInfo(information = "hello")]})]

_base = rule(impl = _base_impl, attrs = {})

def subtarget_test():
    _base(name = "subtarget_base")
    _subtarget(name = "subtarget", base = ":subtarget_base", child = ":subtarget_base[child]")
