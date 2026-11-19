# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolsInfo")
load("@prelude//apple/user:apple_xcframework.bzl", "XCFrameworkInfo")
load("@prelude//utils:utils.bzl", "flatten")

def apple_spm_package_impl(ctx: AnalysisContext) -> list[Provider]:
    apple_tools = ctx.attrs._apple_tools[AppleToolsInfo]

    spm_packager = apple_tools.spm_packager

    output_dir = ctx.actions.declare_output(ctx.attrs.name, dir = True, has_content_based_path = False)

    xcframework_binaries = [dep for dep in ctx.attrs.deps if dep.get(XCFrameworkInfo)]
    binary_args = flatten([["--xcframework", dep[XCFrameworkInfo].name, dep[DefaultInfo].default_outputs[0]] for dep in xcframework_binaries])

    spm_command = cmd_args(
        [
            spm_packager,
            "--output-path",
            output_dir.as_output(),
            "--package-name",
            ctx.attrs.package_name,
        ]
        + binary_args
    )

    ctx.actions.run(spm_command, category = "assemble_spm_package")

    return [
        DefaultInfo(default_output = output_dir),
    ]
