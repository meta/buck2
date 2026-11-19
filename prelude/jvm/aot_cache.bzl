# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")
load(
    "@prelude//jvm:cd_jar_creator_util.bzl",
    "compiler_daemon_base_jvm_args",
    "compiler_daemon_jdk_access_jvm_args",
)
load("@prelude//jvm:launcher_runtime.bzl", "JvmLauncherRuntimeInfo", "aot_jvm_args")

# Fails unless the runtime image's `lib/modules` has the sha256 the cache was trained against, then copies the
# cache to the output.
_VERIFY_SCRIPT = """modules="$1"; expected="$2"; cache="$3"; out="$4"; hint="$5"
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$modules" | cut -d' ' -f1)
else
  actual=$(shasum -a 256 "$modules" | cut -d' ' -f1)
fi
if [ "$actual" != "$expected" ]; then
  echo "The Java AOT cache was trained against a different launcher runtime: its lib/modules has sha256 $actual," >&2
  echo "the cache was trained against $expected. $hint" >&2
  exit 1
fi
cp "$cache" "$out"
"""

def _jvm_aot_cache_impl(ctx: AnalysisContext) -> list[Provider]:
    image = ctx.attrs.launcher_runtime[DefaultInfo].default_outputs[0]
    out = ctx.actions.declare_output(ctx.label.name + ".aot")
    ctx.actions.run(
        cmd_args(
            "/bin/sh",
            "-c",
            _VERIFY_SCRIPT,
            "verify_jvm_aot_cache",
            image.project("lib/modules"),
            ctx.attrs.launcher_runtime_modules_sha256,
            ctx.attrs.cache,
            out.as_output(),
            ctx.attrs.retrain_hint,
        ),
        category = "verify_jvm_aot_cache",
    )
    return [DefaultInfo(default_output = out)]

jvm_aot_cache = rule(
    doc = """
    A Java AOT cache bound to the content of the launcher runtime it was trained with. HotSpot checks only the size of
    the runtime image's `lib/modules` before mapping a cache, so after a same-size change to the image (e.g. to the
    launcher) the JVM would map the cache and run the cached, stale classes. This rule's output is the cache, and
    building it fails unless `lib/modules` has exactly the pinned sha256, so any change to the image fails every
    action that uses the cache until the cache is retrained.
    """,
    impl = _jvm_aot_cache_impl,
    attrs = {
        "cache": attrs.source(doc = "The trained cache file."),
        "launcher_runtime": attrs.dep(providers = [JvmLauncherRuntimeInfo]),
        "launcher_runtime_modules_sha256": attrs.string(doc = "sha256 of the runtime image's `lib/modules` at training time."),
        "retrain_hint": attrs.string(default = "", doc = "Appended to the error when the runtime changed, e.g. how to retrain."),
    },
)

def _jvm_aot_cache_test_impl(ctx: AnalysisContext) -> list[Provider]:
    launcher_runtime = ctx.attrs.launcher_runtime[JvmLauncherRuntimeInfo]
    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]

    # `--dry-run` starts the JVM and loads the launcher's main class without running it. With
    # `-XX:AOTMode=on`, the JVM exits non-zero if it cannot map the cache with the flags compiler daemons
    # start with.
    command = cmd_args(
        launcher_runtime.java,
        compiler_daemon_base_jvm_args(java_toolchain.java_runtime_version),
        compiler_daemon_jdk_access_jvm_args(launcher_runtime),
        aot_jvm_args(ctx.attrs.cache, "on"),
        "--dry-run",
        "-m",
        launcher_runtime.main_module,
    )
    return [
        DefaultInfo(),
        RunInfo(args = command),
        ExternalRunnerTestInfo(
            type = "custom",
            command = [command],
            labels = ctx.attrs.labels,
            contacts = ctx.attrs.contacts,
            run_from_project_root = True,
            use_project_relative_paths = True,
        ),
    ]

jvm_aot_cache_test = rule(
    doc = """
    Fails when a compiler daemon started from `launcher_runtime` cannot map the Java AOT cache `cache`. With a
    `jvm_aot_cache` as `cache`, it also fails, at build time, on any change to the runtime image since training.
    """,
    impl = _jvm_aot_cache_test_impl,
    attrs = {
        "cache": attrs.source(),
        "contacts": attrs.list(attrs.string(), default = []),
        "labels": attrs.list(attrs.string(), default = []),
        "launcher_runtime": attrs.dep(providers = [JvmLauncherRuntimeInfo]),
        "_java_toolchain": toolchains_common.java_for_android(),
    },
)
