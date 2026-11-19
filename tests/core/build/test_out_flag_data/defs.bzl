# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _out_library_impl(ctx):
    outs = []
    for out in ctx.attrs.outs:
        for default_out in out[DefaultInfo].default_outputs:
            out_artifact = ctx.actions.copy_file(default_out.basename, default_out, has_content_based_path = False)
            outs.append(out_artifact)

    return [DefaultInfo(default_outputs = outs)]

out_library = rule(
    impl = _out_library_impl,
    attrs = {
        "outs": attrs.list(attrs.dep()),
    },
)

def _export_file_impl(ctx):
    return [DefaultInfo(default_output = ctx.attrs.src)]

export_file = rule(
    impl = _export_file_impl,
    attrs = {
        "src": attrs.source(allow_directory = True),
    },
)
