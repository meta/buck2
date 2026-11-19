# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _toolchain_impl(_ctx):
    return [
        DefaultInfo(),
        TemplatePlaceholderInfo(
            unkeyed_variables = {
                "FOO": "<foo_compiler>",
                "FOO_FLAGS": "<foo_compiler_flags>",
            },
        ),
    ]

def _library_impl(ctx):
    return [
        DefaultInfo(),
        TemplatePlaceholderInfo(
            keyed_variables = {
                "LIB_FLAGS": cmd_args(ctx.attrs.flags),
                "NAME": ctx.label.name,
            },
        ),
    ]

def _binary_impl(ctx):
    output, _ = ctx.actions.write("out", ctx.attrs.flags, allow_args = True, has_content_based_path = False)
    return [
        DefaultInfo(
            default_output = output,
        ),
    ]

foo_toolchain = rule(
    impl = _toolchain_impl,
    attrs = {},
)

foo_library = rule(
    impl = _library_impl,
    attrs = {
        "flags": attrs.list(attrs.arg()),
    },
)

foo_binary = rule(
    impl = _binary_impl,
    attrs = {
        "flags": attrs.list(attrs.arg()),
        "_toolchains": attrs.list(attrs.dep(), default = ["//:toolchain"]),
    },
)
