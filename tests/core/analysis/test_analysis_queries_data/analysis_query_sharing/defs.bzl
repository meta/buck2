# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _labelled_impl(_ctx):
    return [DefaultInfo()]

labelled = rule(
    impl = _labelled_impl,
    attrs = {
        "deps": attrs.list(attrs.dep(), default = []),
        "labels": attrs.list(attrs.string(), default = []),
    },
)

def _query_impl(ctx):
    out, _ = ctx.actions.write("out", ctx.attrs.arg, allow_args = True, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

query = rule(impl = _query_impl, attrs = {"arg": attrs.arg()})
