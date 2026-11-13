# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _lib_impl(ctx):
    out = ctx.actions.write("out.txt", ctx.attrs.name)
    return [DefaultInfo(default_output = out, sub_targets = {"shared": [DefaultInfo(default_output = out)]})]

lib = rule(
    impl = _lib_impl,
    attrs = {},
)

def _consumer_impl(_ctx):
    return [DefaultInfo()]

consumer = rule(
    impl = _consumer_impl,
    attrs = {
        "dep": attrs.option(attrs.dep(), default = None),
        # Like `python_library.resources`: a string that is not a label falls through to a file.
        "dep_or_file": attrs.option(attrs.one_of(attrs.dep(), attrs.source()), default = None),
        "dep_or_string": attrs.option(attrs.one_of(attrs.dep(), attrs.string()), default = None),
    },
)
