# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")

def _jdk_system_image_impl(ctx: AnalysisContext) -> list[Provider]:
    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]

    output = ctx.actions.declare_output("image", has_content_based_path = False)
    cmd = cmd_args([
        ctx.attrs.create_jdk_system_image[RunInfo],
        "--core-for-system-modules-jar",
        ctx.attrs.core_for_system_modules_jar,
        "--java",
        java_toolchain.java[RunInfo],
        "--javac",
        java_toolchain.javac[RunInfo],
        "--jlink",
        java_toolchain.jlink[RunInfo],
        "--jmod",
        java_toolchain.jmod[RunInfo],
        "--jrt-fs-jar",
        java_toolchain.jrt_fs_jar,
        "--jar-builder",
        ctx.attrs.jar_builder,
        "--output",
        output.as_output(),
    ])
    ctx.actions.run(cmd, category = "create_jdk_system_image")

    return [DefaultInfo(default_output = output)]

jdk_system_image = rule(
    impl = _jdk_system_image_impl,
    attrs = {
        "core_for_system_modules_jar": attrs.source(),
        "create_jdk_system_image": attrs.exec_dep(default = "prelude//android/tools:create_jdk_system_image"),
        "jar_builder": attrs.source(default = "prelude//toolchains/android/src/com/facebook/buck/util/zip:jar_builder"),
        "_java_toolchain": toolchains_common.java_for_android(),
    },
)
