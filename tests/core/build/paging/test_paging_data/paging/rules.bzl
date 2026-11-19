# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _copy_file_impl(ctx):
    out = ctx.actions.copy_file(ctx.attrs.out, ctx.attrs.src, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

copy_file = rule(
    impl = _copy_file_impl,
    attrs = {
        "deps": attrs.list(attrs.dep(), default = []),
        "out": attrs.string(),
        "src": attrs.source(),
    },
)

MyInfo = provider(fields = ["value"])

def _analysis_node_impl(ctx):
    return [DefaultInfo(), MyInfo(value = len(ctx.attrs.deps))]

analysis_node = rule(
    impl = _analysis_node_impl,
    attrs = {
        "deps": attrs.list(attrs.dep(), default = []),
    },
)

# A module-scope frozen value. A rule that references it stores, in its provider, a
# `FrozenValue` that lives on this `.bzl` module's frozen heap rather than the
# target's own analysis heap -- a cross-frozen-heap reference.
_MODULE_CONST = struct(items = ["a", "b", "c"])

def _module_const_impl(_ctx):
    # The context is unused: the provider value comes from the module constant, not the
    # target, which is what puts a foreign-heap `FrozenValue` in the analysis result.
    return [DefaultInfo(), MyInfo(value = _MODULE_CONST)]

module_const = rule(
    impl = _module_const_impl,
    attrs = {},
)

def _anon_impl(ctx):
    out = ctx.actions.write("anon.txt", "anonymous target\n", has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

_anon = rule(
    impl = _anon_impl,
    attrs = {},
)

def _uses_anon_impl(ctx):
    return ctx.actions.anon_target(_anon, {}).promise.map(
        lambda providers: [providers[DefaultInfo]],
    )

uses_anon = rule(
    impl = _uses_anon_impl,
    attrs = {},
)

def _many_actions_impl(ctx):
    for index in range(2000):
        ctx.actions.write(
            "unused-{}.txt".format(index),
            "unused-action-{}".format(index) * 100,
            has_content_based_path = False,
        )
    out = ctx.actions.copy_file("used.txt", ctx.attrs.src, has_content_based_path = False)
    return [
        DefaultInfo(default_output = out),
        InstallInfo(installer = ctx.attrs.installer, files = {"used": out}),
    ]

many_actions = rule(
    impl = _many_actions_impl,
    attrs = {
        "installer": attrs.label(),
        "src": attrs.source(),
    },
)
