# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _binary_impl(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            "touch",
            out.as_output(),
            hidden = ctx.attrs.srcs,
        ),
        category = "test",
    )
    return [DefaultInfo(default_output = out)]

def _library_impl(_ctx):
    return [DefaultInfo()]

my_binary = rule(
    impl = _binary_impl,
    attrs = {
        "deps": attrs.list(attrs.dep(), default = []),
        "srcs": attrs.list(attrs.source(), default = []),
    },
)

my_library = rule(
    impl = _library_impl,
    attrs = {
        "deps": attrs.list(attrs.dep(), default = []),
        "srcs": attrs.list(attrs.source(), default = []),
    },
)

def _cached_binary_impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            "sh",
            "-c",
            "echo 'cached output' > $1",
            "--",
            out.as_output(),
            hidden = ctx.attrs.srcs,
        ),
        category = "cached_test",
        allow_offline_output_cache = True,
    )
    return [DefaultInfo(default_output = out)]

cached_binary = rule(
    impl = _cached_binary_impl,
    attrs = {
        "srcs": attrs.list(attrs.source(), default = []),
    },
)

def _uncached_binary_impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            "sh",
            "-c",
            "echo 'uncached output' > $1",
            "--",
            out.as_output(),
        ),
        category = "uncached_test",
    )
    return [DefaultInfo(default_output = out)]

uncached_binary = rule(impl = _uncached_binary_impl, attrs = {})
