# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _test_impl(ctx):
    out = ctx.actions.declare_output("file", has_content_based_path = False)
    ctx.actions.run(
        ["cp", "/run/re_worker/gang_info", out.as_output()],
        category = "cp",
        env = {"cache_buster": ctx.attrs.cache_buster},
        re_gang_workers = ctx.attrs.re_gang_workers,
    )
    return [DefaultInfo(out)]

test = rule(
    attrs = {
        "cache_buster": attrs.string(default = read_config("test", "cache_buster", "")),
        "re_gang_workers": attrs.list(attrs.dict(key = attrs.string(), value = attrs.string()), default = []),
    },
    impl = _test_impl,
)
