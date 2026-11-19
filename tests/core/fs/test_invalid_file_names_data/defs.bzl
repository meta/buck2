# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _test_impl(ctx):
    out = ctx.actions.symlinked_dir("out", {"in": ctx.attrs.src}, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

test = rule(
    impl = _test_impl,
    attrs = {"src": attrs.source(allow_directory = True)},
)

def defs():
    test(name = "root", src = "src")
