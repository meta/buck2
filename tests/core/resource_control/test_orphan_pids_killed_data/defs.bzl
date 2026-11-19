# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

_cache_buster = read_config("test", "cache_buster", default = "")

def _impl(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            "sh",
            "-c",
            '(setsid sleep 300 </dev/null &>/dev/null &) && echo done > "$1"',
            "--",
            out.as_output(),
        ),
        env = {"CACHE_BUSTER": _cache_buster},
        category = "spawn_orphan",
        local_only = True,
    )
    return [DefaultInfo(default_output = out)]

spawn_orphan = rule(
    impl = _impl,
    attrs = {},
)

def _timeout_impl(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            "sh",
            "-c",
            # Spawn a background process in the SAME process group, then block.
            # When the timeout fires, killpg kills both the main shell and the
            # background sleep, so cgroup cleanup should find nothing left.
            "(sleep 300 &) && sleep 300",
            "--",
            out.as_output(),
        ),
        env = {"CACHE_BUSTER": _cache_buster},
        category = "spawn_same_pg_timeout",
        local_only = True,
        timeout_seconds = 3,
    )
    return [DefaultInfo(default_output = out)]

spawn_same_pg_timeout = rule(
    impl = _timeout_impl,
    attrs = {},
)
