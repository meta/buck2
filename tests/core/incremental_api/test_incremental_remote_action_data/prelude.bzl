# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _china_impl(ctx) -> list[Provider]:
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["fbpython", ctx.attrs.increment] + (["--dir"] if ctx.attrs.dir else []) + ["--out", out.as_output()]),
        category = "increment",
        no_outputs_cleanup = True,
        incremental_remote_outputs = True,
        prefer_remote = True,
        env = {"INVALIDATE_ACTION": ctx.attrs.invalidate},
    )
    return [
        DefaultInfo(out),
        RunInfo(args = ["cat", cmd_args(out, format = "{}/MYFILE") if ctx.attrs.dir else out]),
    ]

china = rule(
    impl = _china_impl,
    attrs = {
        "dir": attrs.bool(default = False),
        "increment": attrs.source(),
        "invalidate": attrs.string(),
    },
)

def _whistle_impl(ctx) -> list[Provider]:
    intermediate = ctx.actions.declare_output("intermediate", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["fbpython", ctx.attrs.increment] + (["--dir"] if ctx.attrs.dir else []) + ["--out", intermediate.as_output()]),
        category = "increment",
        no_outputs_cleanup = True,
        incremental_remote_outputs = True,
        prefer_remote = True,
        env = {"INVALIDATE_ACTION": ctx.attrs.invalidate},
    )
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["sh", "-c", 'cp -rf "$1" "$2"', "--", intermediate, out.as_output()]),
        category = "copy",
        prefer_remote = True,
        env = {"INVALIDATE_ACTION": ctx.attrs.invalidate},
    )
    return [
        DefaultInfo(out),
        RunInfo(args = ["cat", cmd_args(out, format = "{}/MYFILE") if ctx.attrs.dir else out]),
    ]

whistle = rule(
    impl = _whistle_impl,
    attrs = {
        "dir": attrs.bool(default = False),
        "increment": attrs.source(),
        "invalidate": attrs.string(),
    },
)
