# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _dep_impl(ctx):
    out = ctx.actions.declare_output("dep", has_content_based_path = ctx.attrs.use_content_based_path)
    ctx.actions.run(
        [
            "fbpython",
            ctx.attrs.script,
            out.as_output(),
        ],
        env = {"cache_buster": ctx.attrs.cache_buster},
        category = "test",
    )
    return [DefaultInfo(default_output = out)]

dep = rule(
    impl = _dep_impl,
    attrs = {
        "cache_buster": attrs.string(default = read_config("test", "cache_buster", "")),
        "script": attrs.source(),
        "use_content_based_path": attrs.bool(default = read_config("test", "use_content_based_path", "") in ["true", "True"]),
    },
)

def _action_fail(ctx):
    dep = ctx.attrs.dep[DefaultInfo].default_outputs[0]
    out = ctx.actions.declare_output("failed_action", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            "fbpython",
            "-c",
            "import sys; sys.exit(1)",
            out.as_output(),
            hidden = dep,
        ),
        env = {"cache_buster": ctx.attrs.cache_buster},
        category = "test",
    )
    return [DefaultInfo(default_outputs = [out])]

action_fail = rule(
    impl = _action_fail,
    attrs = {
        "cache_buster": attrs.string(default = read_config("test", "cache_buster", "")),
        "dep": attrs.dep(),
    },
)
