# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@toolchains//:rust_toolchain.bzl", "RustCompilerInfo")

def _rust_binary_impl(ctx):
    file = ctx.attrs.file
    extension = ".exe" if host_info().os.is_windows else ""
    out = ctx.actions.declare_output("main" + extension, has_content_based_path = False)

    cmd = cmd_args([ctx.attrs.toolchain[RustCompilerInfo].compiler_path, "--crate-type=bin", file, "-o", out.as_output()])

    ctx.actions.run(cmd, category = "compile")

    return [DefaultInfo(default_output = out), RunInfo(args = cmd_args([out]))]

rust_binary = rule(
    impl = _rust_binary_impl,
    attrs = {
        "file": attrs.source(),
        "toolchain": attrs.toolchain_dep(),
    },
)
