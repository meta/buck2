# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl(ctx):
    a = ctx.actions.declare_output("a", dir = True, has_content_based_path = False)
    z = ctx.actions.declare_output("z", dir = True, has_content_based_path = False)
    cmd = ["sh", "-c", 'mkdir -p "$1" "$2" && touch "$1"/foo', "--", z.as_output(), a.as_output()]
    ctx.actions.run(
        cmd,
        category = "mkdir",
        env = {"CACHE_BUSTER": ctx.attrs.cache_buster},
    )
    return [DefaultInfo(a, other_outputs = [z])]

my_rule = rule(
    impl = _impl,
    attrs = {
        "cache_buster": attrs.string(default = read_config("test", "cache_buster", "")),
    },
)
