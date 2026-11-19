# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

_use_some_memory = read_config("use_some_memory", "path")

_allocate_mb = read_config("use_some_memory", "allocate_mb")

_cache_buster = read_config("test", "cache_buster", default = "")

def _impl(ctx):
    usm_bin = ctx.actions.declare_output("usm.bin", has_content_based_path = False)
    ctx.actions.run(
        # Copy the helper binary into buck-out so the action below can
        # run remotely
        cmd_args(
            "cp",
            _use_some_memory,
            usm_bin.as_output(),
        ),
        local_only = True,
        allow_cache_upload = False,
        category = "copy",
    )

    args = cmd_args(
        usm_bin,
        "--allocate-count",
        "1",
        "--each-tick-allocate-memory",
        _allocate_mb,
    )

    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            args,
            "--output",
            out.as_output(),
        ),
        env = {"CACHE_BUSTER": _cache_buster},
        category = "use_some_memory",
    )
    return [
        DefaultInfo(default_output = out),
        ExternalRunnerTestInfo(
            command = [args],
            type = "custom",
        ),
    ]

use_some_memory = rule(
    impl = _impl,
    attrs = {},
)
