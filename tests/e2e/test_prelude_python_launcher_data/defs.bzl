# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")

def _inplace_launcher_impl(ctx):
    runtime = ctx.attrs._runtime[DefaultInfo].default_outputs[0]
    link_tree = ctx.actions.symlinked_dir(
        ctx.attrs.name + "#link-tree",
        {
            "__par__": runtime.project("__par__"),
            ctx.attrs.main.basename: ctx.attrs.main,
        },
    )
    out = ctx.actions.declare_output(ctx.attrs.name + ".pex")

    # The arguments `_pex_bootstrap_args` in make_py_package.bzl passes for an inplace package.
    cmd = cmd_args(
        ctx.attrs._tool[RunInfo],
        "--python",
        ctx.attrs.interpreter,
        "--host-python",
        ctx.attrs.interpreter,
        "--entry-point",
        paths.replace_extension(ctx.attrs.main.basename, ""),
        cmd_args(["--modules-dir", link_tree], ignore_artifacts = True),
        "--main-runner",
        "__par__.bootstrap.run_as_main",
        out.as_output(),
    )
    ctx.actions.run(cmd, category = "par", identifier = "bootstrap")
    return [
        DefaultInfo(default_output = out, other_outputs = [link_tree]),
        RunInfo(args = cmd_args(out, hidden = [link_tree])),
    ]

# An inplace launcher for one module, written by the tool `make_py_package.bzl` runs for a
# `python_binary` of package style `inplace`, over the prelude's own `__par__` runtime.
inplace_launcher = rule(
    impl = _inplace_launcher_impl,
    attrs = {
        "interpreter": attrs.string(),
        "main": attrs.source(),
        "_runtime": attrs.default_only(attrs.dep(default = "prelude//python/runtime:bootstrap_files")),
        "_tool": attrs.default_only(attrs.exec_dep(providers = [RunInfo], default = "prelude//python/tools:make_py_package_inplace")),
    },
)
