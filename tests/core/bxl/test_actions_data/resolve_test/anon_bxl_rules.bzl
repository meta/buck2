# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

MirrorInfo = provider(fields = ["info"])

def assert_eq(a, b):
    if a != b:
        fail("Expected {} == {}".format(a, b))

def _mirror_impl(ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo(), MirrorInfo(info = ctx.attrs)]

mirror = rule(
    impl = _mirror_impl,
    attrs = {
        "dep": attrs.dep(),
        "false": attrs.bool(),
        "int": attrs.int(),
        "list_string": attrs.list(attrs.string()),
        "string": attrs.string(),
        "true": attrs.bool(),
    },
)

ValidateInfo = provider(fields = ["string"])
