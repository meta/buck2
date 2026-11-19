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

def _impl(ctx):
    return [DefaultInfo(), FooInfo(foo = ctx.attrs.name + "_foo")]

def _binary_impl(ctx):
    return [DefaultInfo(), RunInfo(args = []), FooInfo(foo = ctx.attrs.name + "_foo")]

def _buildable_impl(ctx):
    out = ctx.actions.write(ctx.attrs.out, ctx.attrs.content, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

_foo_library = rule(
    impl = _impl,
    attrs = {
        "cmd": attrs.list(attrs.arg(), default = []),
        "deps": attrs.list(attrs.dep(), default = []),
        "description": attrs.string(default = ""),
        "mapped_srcs": attrs.dict(attrs.string(), attrs.source(), default = {}),
        "srcs": attrs.list(attrs.source(), default = []),
        "tuple_srcs": attrs.option(attrs.tuple(attrs.source(), attrs.source(), attrs.source()), default = None),
    },
)

_foo_binary = rule(
    impl = _binary_impl,
    attrs = {
        "cmd": attrs.list(attrs.arg(), default = []),
        "deps": attrs.list(attrs.dep(), default = []),
        "description": attrs.string(default = ""),
        "srcs": attrs.list(attrs.source(), default = []),
        "_foo_toolchain": attrs.exec_dep(default = "root//:foo_toolchain"),
    },
)

_foo_genrule = rule(
    impl = _binary_impl,
    attrs = {
        "cmd": attrs.arg(),
        "description": attrs.string(default = ""),
        "out": attrs.string(default = ""),
    },
)

_foo_buildable = rule(
    impl = _buildable_impl,
    attrs = {
        "content": attrs.string(default = ""),
        "out": attrs.string(),
    },
)

_default_platform = "root//platforms:platform1"

def _basic_print_impl(ctx):
    _ignore = ctx  # buildifier: disable=unused-variable

    print("print me")  # buildifier: disable=print
    return [DefaultInfo(), RunInfo(args = [])]

foo_basic_print = rule(
    impl = _basic_print_impl,
    attrs = {},
)

def foo_library(**kwargs):
    _foo_library(default_target_platform = _default_platform, **kwargs)

def foo_binary(**kwargs):
    _foo_binary(default_target_platform = _default_platform, **kwargs)

def foo_genrule(**kwargs):
    _foo_genrule(default_target_platform = _default_platform, **kwargs)

def foo_buildable(**kwargs):
    _foo_buildable(default_target_platform = _default_platform, **kwargs)

def genrule_select() -> Select:
    return select({
        "DEFAULT": "foo",
        "ovr_config//os:macos": "bar",
        "ovr_config//os:windows": "foobar",
    })
