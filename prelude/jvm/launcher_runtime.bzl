# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")

JvmLauncherRuntimeInfo = provider(
    doc = """
    A JDK runtime image with a launcher jar linked in as a named module. Launching with
    `java -m <main_module>` leaves the application classpath empty, so a Java AOT cache trained
    with this runtime does not depend on the path or mtime of any file outside the image and loads
    wherever the image is materialized.
    """,
    fields = {
        # The image's `bin/java` with the launch flags, and the whole image as a hidden input.
        "java": provider_field(RunInfo),
        # `<module>/<main class>`, the argument to `-m`.
        "main_module": provider_field(str),
        "module": provider_field(str),
    },
)

def aot_jvm_args(aot_cache: Artifact | None, aot_mode: str = "on") -> list:
    """JVM arguments that map `aot_cache`, a Java AOT cache trained through a launcher runtime."""
    if not aot_cache:
        return []
    return ["-XX:AOTMode={}".format(aot_mode), cmd_args(aot_cache, format = "-XX:AOTCache={}")]

def in_aot_cache_arm(qualified_name: str, percent: int | None) -> bool:
    """
    Whether actions of the target `qualified_name` (`cell//path:name`, with or without a `[subtarget]`) get the AOT
    cache when only `percent` percent of targets do, for an A/B comparison within the same builds. All of a target's
    actions are in the same arm, and the split is stable across builds and hosts.

    A target is in the cache arm when `hash(name) % 100 < percent`. Starlark's `hash` of a string is Java's
    `String.hashCode`, a signed 32-bit value, and Starlark's `%` takes the sign of the divisor, so the bucket is
    always in [0, 100). To recompute the arm offline, use the same floored modulo: `Math.floorMod(name.hashCode(),
    100)` in Java or `h % 100` in Python. Java's `%` keeps the dividend's sign and puts every target with a negative
    hash in the cache arm.
    """
    if percent == None:
        return True
    return hash(qualified_name.split("[")[0]) % 100 < percent

def _jvm_launcher_runtime_impl(ctx: AnalysisContext) -> list[Provider]:
    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    image = ctx.actions.declare_output("runtime", dir = True, has_content_based_path = False)
    ctx.actions.run(
        cmd_args([
            ctx.attrs._builder[RunInfo],
            "--java",
            java_toolchain.java[RunInfo],
            "--javac",
            java_toolchain.javac[RunInfo],
            "--jlink",
            java_toolchain.jlink[RunInfo],
            "--launcher-jar",
            ctx.attrs.launcher_jar,
            "--module",
            ctx.attrs.module,
            "--output",
            image.as_output(),
        ]),
        category = "jvm_launcher_runtime",
    )
    java = RunInfo(
        args = cmd_args(
            image.project("bin/java"),
            # `-m` alone resolves only the launcher module's dependencies. `ALL-DEFAULT` adds the root modules a
            # classpath launch resolves (e.g. `jdk.unsupported`), which code loaded from the unnamed module expects.
            "--add-modules",
            "ALL-DEFAULT",
            # Keep machine code out of AOT caches: JDK 25 does not check that the CPU replaying cached adapters
            # supports the instructions they were generated with, and they crash with SIGILL on older hosts.
            # TODO(jselbo): this looks like a JDK bug; consider reporting it to OpenJDK.
            "-XX:+UnlockDiagnosticVMOptions",
            "-XX:-AOTAdapterCaching",
            "-XX:-AOTStubCaching",
            # JDK 25 does not clean archived training profiles of classes that are unloaded before the
            # profile's method is restored, and C1 crashes reading them. Compiler daemons load the same
            # classes in several short-lived class loaders, and their JVMs are too short-lived for class
            # unloading to matter. TODO(jselbo): drop once the JDK cleans those profiles.
            "-XX:-ClassUnloading",
            hidden = image,
        ),
    )
    return [
        DefaultInfo(default_output = image),
        java,
        JvmLauncherRuntimeInfo(
            main_module = "{}/{}".format(ctx.attrs.module, ctx.attrs.main_class),
            module = ctx.attrs.module,
            java = java,
        ),
    ]

jvm_launcher_runtime = rule(
    impl = _jvm_launcher_runtime_impl,
    attrs = {
        "launcher_jar": attrs.source(doc = "Jar holding the launcher's main class and everything it loads from the system class loader."),
        "main_class": attrs.string(),
        "module": attrs.string(doc = "Name of the module the launcher jar becomes."),
        "_builder": attrs.exec_dep(default = "prelude//java/tools:jvm_launcher_runtime", providers = [RunInfo]),
        "_java_toolchain": toolchains_common.java_bootstrap(),
    },
)
