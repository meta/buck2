# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _cp_impl(ctx):
    return [DefaultInfo(default_output = ctx.actions.copy_file("out", ctx.attrs.src))]

cp = rule(impl = _cp_impl, attrs = {"src": attrs.source()})

def _files_impl(ctx):
    return [DefaultInfo(default_outputs = ctx.attrs.srcs)]

files = rule(impl = _files_impl, attrs = {"srcs": attrs.list(attrs.source())})
