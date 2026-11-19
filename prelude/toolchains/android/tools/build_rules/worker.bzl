# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo", "unsafe_memory_access_jvm_args")
load("@prelude//toolchains/android/tools:build_rules.bzl", "OPEN_JDK_COMPILER_ARGS")

def _jvm_arg_name_is_specified(arg_name: str, existing_jvm_args: list[str]) -> bool:
    for existing_jvm_arg in existing_jvm_args:
        if existing_jvm_arg.startswith(arg_name):
            return True

    return False

def _get_jvm_args_for_worker(ctx, java_toolchain: JavaToolchainInfo):
    jvm_args = ctx.attrs.jvm_args
    if not "-XX:-MaxFDLimit" in jvm_args:
        jvm_args.insert(0, "-XX:-MaxFDLimit")
    if not _jvm_arg_name_is_specified("-Dfile.encoding", jvm_args):
        jvm_args.insert(0, "-Dfile.encoding=UTF-8")
    if not _jvm_arg_name_is_specified("-XX:CompressedClassSpaceSize", jvm_args):
        jvm_args.insert(0, "-XX:CompressedClassSpaceSize=2g")

    # Pre-size JIT code cache and metaspace to reduce warmup overhead on cold workers.
    # kotlincd loads the Kotlin compiler, KSP framework, and annotation processors,
    # which generates significant JIT-compiled code. Default code cache (240MB) can
    # fill with 16 concurrent compilations, causing eviction and re-compilation.
    # Default metaspace (~20MB) triggers repeated full GC as classes are loaded.
    if not _jvm_arg_name_is_specified("-XX:ReservedCodeCacheSize", jvm_args):
        jvm_args.insert(0, "-XX:ReservedCodeCacheSize=512m")
    if not _jvm_arg_name_is_specified("-XX:MetaspaceSize", jvm_args):
        jvm_args.insert(0, "-XX:MetaspaceSize=256m")

    # Allow JVM compiler daemon to access internal jdk.compiler APIs
    jvm_args.extend(OPEN_JDK_COMPILER_ARGS)
    jvm_args.append("--add-opens=java.base/java.util=ALL-UNNAMED")

    jvm_args.extend(unsafe_memory_access_jvm_args(java_toolchain.java_runtime_version))

    return jvm_args

def _get_args_for_worker(ctx):
    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    args = cmd_args()
    args.add(java_toolchain.java[RunInfo])
    args.add(_get_jvm_args_for_worker(ctx, java_toolchain))
    args.add([
        "-cp",
        ctx.attrs.exe,
        "-jar",
        ctx.attrs.class_loader_bootstrapper,
        ctx.attrs.main_class,
    ])

    return args

def _worker_impl(ctx):
    args = _get_args_for_worker(ctx)
    return [
        DefaultInfo(),
        WorkerInfo(
            exe = RunInfo(args),
            env = {
                "BUCK_CLASSPATH": ctx.attrs.exe,
                "JAVACD_ABSOLUTE_PATHS_ARE_RELATIVE_TO_CWD": "1",
            },
        ),
    ]

worker = rule(
    impl = _worker_impl,
    attrs = {
        "class_loader_bootstrapper": attrs.source(default = "prelude//toolchains/android/src/com/facebook/buck/cli/bootstrapper:bootstrapper"),
        "concurrency": attrs.option(attrs.int(), default = None),
        "exe": attrs.source(),
        "jvm_args": attrs.list(attrs.string(), default = []),
        "main_class": attrs.string(),
        "_java_toolchain": toolchains_common.java_bootstrap(),
    },
)

remote_worker = rule(
    impl = _worker_impl,
    attrs = {
        "class_loader_bootstrapper": attrs.source(default = "prelude//toolchains/android/src/com/facebook/buck/cli/bootstrapper:bootstrapper"),
        "concurrency": attrs.option(attrs.int(), default = None),
        "exe": attrs.source(),
        "jvm_args": attrs.list(attrs.string(), default = []),
        "main_class": attrs.string(),
        "_java_toolchain": toolchains_common.java_bootstrap(),
    },
)
