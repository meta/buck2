# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

FooInfo = provider(
    fields = {
        "foo": str,
    }
)

def _provides_foo(ctx):
    return [
        DefaultInfo(sub_targets = {"child_{}".format(i): [DefaultInfo(), FooInfo(foo = "child_{}".format(i))] for i in range(130)}),
        FooInfo(foo = ctx.attrs.name + "_foo"),
    ]

provides_foo = rule(
    impl = _provides_foo,
    attrs = {},
)
