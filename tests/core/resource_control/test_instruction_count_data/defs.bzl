# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

_bin_path = read_config("three_billion_instructions", "path")

_cache_buster = read_config("test", "cache_buster", default = "")

def _impl(ctx):
    tbi_bin = ctx.actions.declare_output("tbi.bin", has_content_based_path = False)
    ctx.actions.run(
        # Copy the helper binary into buck-out so the action below can
        # run remotely
        cmd_args(
            "cp",
            _bin_path,
            tbi_bin.as_output(),
        ),
        local_only = True,
        allow_cache_upload = False,
        category = "copy",
    )
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            "bash",
            "-c",
            cmd_args(
                tbi_bin,
                "&&",
                "touch",
                out.as_output(),
                delimiter = " ",
            ),
        ),
        env = {"CACHE_BUSTER": _cache_buster},
        category = "three_billion_instructions",
    )
    return [
        DefaultInfo(default_output = out),
    ]

three_billion_instructions = rule(
    impl = _impl,
    attrs = {},
)
