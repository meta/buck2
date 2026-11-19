# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _copy_file_impl(ctx):
    test = ctx.attrs.test or ctx.attrs.name
    if test == "uses_declared_output":
        declared = ctx.actions.declare_output(ctx.attrs.out, has_content_based_path = False)
        output = ctx.actions.copy_file(declared, ctx.attrs.src)
        return [DefaultInfo(default_output = output)]
    elif test == "uses_declared_output_as_output":
        declared = ctx.actions.declare_output(ctx.attrs.out, has_content_based_path = False)
        output = ctx.actions.copy_file(declared.as_output(), ctx.attrs.src)
        return [DefaultInfo(default_output = output)]
    elif test == "declares_output":
        output = ctx.actions.copy_file(ctx.attrs.out, ctx.attrs.src, has_content_based_path = False)
        return [DefaultInfo(default_output = output)]
    elif test == "fails_on_invalid_src":
        ctx.actions.copy_file(ctx.attrs.out, [], has_content_based_path = False)
        fail("should fail in copy() function")
    elif test == "fails_on_invalid_dest":
        ctx.actions.copy_file([], ctx.attrs.src, has_content_based_path = False)
        fail("should fail in copy() function")
    else:
        fail("invalid test")

copy_file = rule(
    impl = _copy_file_impl,
    attrs = {
        "out": attrs.string(),
        "src": attrs.source(),
        "test": attrs.option(attrs.string(), default = None),
    },
)
