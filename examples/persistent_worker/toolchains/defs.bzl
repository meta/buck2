# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//python:toolchain.bzl",
    "PythonPlatformInfo",
    "PythonToolchainInfo",
)

def _distribution_interpreter(ctx):
    distribution = ctx.attrs.distribution[DefaultInfo].default_outputs[0]
    return cmd_args(distribution, absolute_suffix = "/python/bin/python")

def _python_interpreter_impl(ctx):
    return [
        DefaultInfo(),
        RunInfo(args = [_distribution_interpreter(ctx)]),
    ]

# The interpreter from a Python distribution, as a runnable target. The
# bootstrap toolchain runs prelude's own Python tools with it, so they do not
# depend on whichever `python3` the executor's image happens to provide.
python_interpreter = rule(
    impl = _python_interpreter_impl,
    attrs = {
        "distribution": attrs.exec_dep(),
    },
)

def _python_toolchain_impl(ctx):
    interpreter = _distribution_interpreter(ctx)
    return [
        DefaultInfo(),
        PythonToolchainInfo(
            binary_linker_flags = ctx.attrs.binary_linker_flags,
            linker_flags = ctx.attrs.linker_flags,
            host_interpreter = RunInfo(args = [interpreter]),
            interpreter = RunInfo(args = [interpreter]),
            compile = RunInfo(args = ["echo", "COMPILEINFO"]),
            package_style = "inplace",
            pex_extension = ctx.attrs.pex_extension,
            native_link_strategy = "separate",
        ),
        PythonPlatformInfo(name = "x86_64"),
    ]

python_toolchain = rule(
    impl = _python_toolchain_impl,
    attrs = {
        "binary_linker_flags": attrs.default_only(attrs.list(attrs.arg(), default = [])),
        "distribution": attrs.exec_dep(),
        "linker_flags": attrs.default_only(attrs.list(attrs.arg(), default = [])),
        "pex_extension": attrs.string(default = ".pex"),
    },
    is_toolchain_rule = True,
)
