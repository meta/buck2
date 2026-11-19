# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# @nolint

def _genrule(ctx):
    _ignore = ctx
    fail("not needed in this test")

genrule = rule(
    impl = _genrule,
    attrs = {
        "cmd": attrs.arg(),
        "out": attrs.string(),
    },
)

def _sh_binary(ctx):
    _ignore = ctx
    fail("not needed in this test")

sh_binary = rule(
    impl = _sh_binary,
    attrs = {
        "main": attrs.source(),
    },
)
