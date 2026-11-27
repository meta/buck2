# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load(
    "@prelude//java:java_toolchain.bzl",
    "AbiGenerationMode",
    "DepFiles",
    "JavaPlatformInfo",
    "JavaTestToolchainInfo",
    "JavaToolchainInfo",
    "JavacProtocol",
    "PrebuiltJarToolchainInfo",
)
load("@prelude//tests:test_listing.bzl", "TestListingInfo")

_CP_SNAPSHOT_GENERATOR = "prelude//toolchains/android/src/com/facebook/buck/jvm/kotlin/buildtools/snapshot:cp_snapshot_generator"
_JAVACD_WORKER = "prelude//toolchains/android/src/com/facebook/buck/jvm/java/stepsbuilder/javacd/worker:javacd_grpc_worker_tool"
_CLASS_ABI_GENERATOR = "prelude//toolchains/android/src/com/facebook/buck/jvm/java/abi:api-stubber"

def _read_bool(section, key, default):
    v = read_root_config(section, key, None)
    if v == None:
        return default
    return v.lower() in ("true", "1", "yes", "on")

def use_persistent_workers():
    """`[build] use_persistent_workers`, on by default as in Meta's toolchains."""
    return _read_bool("build", "use_persistent_workers", True)

def _system_java_tool_impl(ctx):
    return [
        DefaultInfo(),
        RunInfo([ctx.attrs.tool_name]),
    ]

system_java_tool = rule(
    impl = _system_java_tool_impl,
    attrs = {
        "tool_name": attrs.string(),
    },
)

def _system_java_lib_impl(ctx):
    if ctx.attrs.jar:
        output = ctx.actions.declare_output(paths.basename(ctx.attrs.jar), has_content_based_path = False)
        ctx.actions.run(cmd_args(["ln", "-s", ctx.attrs.jar, output.as_output()]), category = "{}_symlink".format(ctx.attrs.name))
    else:
        output = ctx.actions.declare_output("jrt-fs.jar", has_content_based_path = False)
        ctx.actions.run(
            cmd_args([
                "sh",
                "-c",
                'ln -s "$(java -XshowSettings:properties -version 2>&1 | sed -n "s/^ *java.home = //p")/lib/jrt-fs.jar" "$1"',
                "--",
                output.as_output(),
            ]),
            category = "{}_symlink".format(ctx.attrs.name),
            local_only = True,
        )
    return [DefaultInfo(default_output = output)]

system_java_lib = rule(
    impl = _system_java_lib_impl,
    attrs = {
        # Absolute path of the jar. Without it, the `jrt-fs.jar` of the JDK whose `java` is on `PATH`.
        "jar": attrs.option(attrs.string(), default = None),
    },
)

def system_prebuilt_jar_bootstrap_toolchain(name, java, generate_snapshot = False, visibility = None):
    """Bootstrap prebuilt_jar toolchain. `generate_snapshot` adds Kotlin classpath snapshots, which
    Kotlin incremental compilation needs for every classpath entry (the toolchain's own jars too)."""
    kwargs = {}
    if generate_snapshot:
        kwargs["cp_snapshot_generator"] = _CP_SNAPSHOT_GENERATOR

    _prebuilt_jar_toolchain_rule(name = name, java = java, visibility = visibility, **kwargs)

def system_prebuilt_jar_toolchain(name, java, visibility = None):
    """prebuilt_jar toolchain for app code: ABI jars and Kotlin classpath snapshots of prebuilts."""
    _prebuilt_jar_toolchain_rule(
        name = name,
        java = java,
        class_abi_generator = _CLASS_ABI_GENERATOR,
        consumer_proguard_rules_extractor = "prelude//java/tools:extract_proguard_rules",
        cp_snapshot_generator = _CP_SNAPSHOT_GENERATOR,
        is_bootstrap_toolchain = False,
        visibility = visibility,
    )

def _prebuilt_jar_toolchain_rule_impl(ctx):
    return [
        DefaultInfo(),
        PrebuiltJarToolchainInfo(
            class_abi_generator = ctx.attrs.class_abi_generator,
            consumer_proguard_rules_extractor = ctx.attrs.consumer_proguard_rules_extractor,
            cp_snapshot_generator = ctx.attrs.cp_snapshot_generator,
            global_code_config = {},
            is_bootstrap_toolchain = ctx.attrs.is_bootstrap_toolchain,
            java = ctx.attrs.java,
        ),
    ]

_prebuilt_jar_toolchain_rule = rule(
    attrs = {
        "class_abi_generator": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "consumer_proguard_rules_extractor": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "cp_snapshot_generator": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "is_bootstrap_toolchain": attrs.bool(default = True),
        # this just goes into the RunInfo of the result, so it's target, not exec
        "java": attrs.dep(providers = [RunInfo]),
    },
    impl = _prebuilt_jar_toolchain_rule_impl,
    is_toolchain_rule = True,
)

def javacd_toolchain(name, java, javac, jar, jlink, jmod, jrt_fs_jar, java_for_tests = None, visibility = None):
    _java_toolchain(
        name = name,
        visibility = visibility,
        java = java,
        jar = jar,
        java_for_tests = java_for_tests,
        is_bootstrap_toolchain = False,
        # The highest ABI generation mode of any target: its own `abi_generation_mode` can only lower it.
        abi_generation_mode = read_root_config("java", "abi_generation_mode", "class"),
        class_abi_generator = "prelude//toolchains/android/src/com/facebook/buck/jvm/java/abi:api-stubber",
        class_loader_bootstrapper = "prelude//toolchains/android/src/com/facebook/buck/cli/bootstrapper:bootstrapper",
        fat_jar_main_class_lib = "prelude//toolchains/android/src/com/facebook/buck/jvm/java/fatjar:fat-jar-main-binary",
        javac = javac,
        javacd = "prelude//toolchains/android/src/com/facebook/buck/jvm/java/stepsbuilder/javacd/main:javacd_tool",
        javac_protocol = "javacd",
        javacd_main_class = "com.facebook.buck.jvm.java.stepsbuilder.javacd.main.JavaCDMain",
        # Incremental-build optimisations, on by default as in Meta's production toolchain.
        # Each one has a buckconfig kill switch with the same key Meta uses.
        cp_snapshot_generator = _CP_SNAPSHOT_GENERATOR,
        dep_files = read_root_config("java", "javacd_dep_files", "per_class"),
        javacd_worker = _JAVACD_WORKER if use_persistent_workers() else None,
        track_class_usage = _read_bool("java", "track_class_usage", True),
        jlink = jlink,
        jmod = jmod,
        jrt_fs_jar = jrt_fs_jar,
    )

def system_java_bootstrap_toolchain(name, java, javac, jlink, jmod, jrt_fs_jar, visibility = None):
    _java_toolchain(
        name = name,
        visibility = visibility,
        java = java,
        is_bootstrap_toolchain = True,
        javac = javac,
        javac_protocol = "classic",
        jlink = jlink,
        jmod = jmod,
        jrt_fs_jar = jrt_fs_jar,
    )

def _java_toolchain_impl(ctx):
    return [
        DefaultInfo(),
        JavaPlatformInfo(
            name = ctx.attrs.name,
        ),
        JavaToolchainInfo(
            abi_generation_mode = AbiGenerationMode(ctx.attrs.abi_generation_mode),
            compile_and_package = ctx.attrs.compile_and_package,
            class_abi_generator = ctx.attrs.class_abi_generator,
            class_loader_bootstrapper = ctx.attrs.class_loader_bootstrapper,
            cp_snapshot_generator = ctx.attrs.cp_snapshot_generator,
            dep_files = DepFiles(ctx.attrs.dep_files),
            fat_jar_main_class_lib = ctx.attrs.fat_jar_main_class_lib,
            compiler_class_to_source_map = ctx.attrs.compiler_class_to_source_map,
            gen_class_to_source_map = ctx.attrs.gen_class_to_source_map,
            gen_class_to_source_map_include_sourceless_compiled_packages = ctx.attrs.gen_class_to_source_map_include_sourceless_compiled_packages,
            gen_class_to_source_map_debuginfo = None,
            fat_jar = ctx.attrs.fat_jar,
            is_bootstrap_toolchain = ctx.attrs.is_bootstrap_toolchain,
            jar = ctx.attrs.jar[RunInfo] if ctx.attrs.jar else None,
            java = ctx.attrs.java,
            java_for_tests = ctx.attrs.java_for_tests[RunInfo] if ctx.attrs.java_for_tests else ctx.attrs.java[RunInfo],
            javac = ctx.attrs.javac,
            javacd = ctx.attrs.javacd,
            javac_protocol = ctx.attrs.javac_protocol,
            javacd_jvm_args = [],
            javacd_jvm_args_target = [],
            javacd_main_class = ctx.attrs.javacd_main_class,
            javacd_worker = ctx.attrs.javacd_worker,
            jar_builder = RunInfo(cmd_args([ctx.attrs.java[RunInfo], "-jar", ctx.attrs.jar_builder])),
            jlink = ctx.attrs.jlink,
            jmod = ctx.attrs.jmod,
            jrt_fs_jar = ctx.attrs.jrt_fs_jar,
            src_root_elements = [],
            src_root_prefixes = [],
            track_class_usage = ctx.attrs.track_class_usage,
            zip_scrubber = RunInfo(cmd_args([ctx.attrs.java[RunInfo], "-jar", ctx.attrs.zip_scrubber])),
            nullsafe = None,
            proguard_max_heap_size = ctx.attrs.proguard_max_heap_size,
            nullsafe_extra_args = [],
            nullsafe_signatures = None,
            semanticdb_javac = None,
            semanticdb_sourceroot = None,
            global_code_config = {},
            merge_class_to_source_maps = ctx.attrs.merge_class_to_source_maps,
            source_level = ctx.attrs.source_level,
            target_level = ctx.attrs.target_level,
        ),
    ]

_java_toolchain = rule(
    impl = _java_toolchain_impl,
    is_toolchain_rule = True,
    attrs = {
        "abi_generation_mode": attrs.enum(["class", "none", "source", "source_only"], default = "none"),
        "class_abi_generator": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "class_loader_bootstrapper": attrs.option(attrs.source(), default = None),
        "compile_and_package": attrs.exec_dep(
            default = "prelude//java/tools:compile_and_package",
            providers = [RunInfo],
        ),
        "compiler_class_to_source_map": attrs.exec_dep(
            default = "prelude//java/tools:compiler_class_to_source_map",
            providers = [RunInfo],
        ),
        "cp_snapshot_generator": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "dep_files": attrs.enum(["none", "per_class", "per_jar"], default = "none"),
        "fat_jar": attrs.exec_dep(
            default = "prelude//java/tools:fat_jar",
            providers = [RunInfo],
        ),
        "fat_jar_main_class_lib": attrs.option(attrs.source(), default = None),
        "gen_class_to_source_map": attrs.exec_dep(
            default = "prelude//java/tools:gen_class_to_source_map",
            providers = [RunInfo],
        ),
        "gen_class_to_source_map_include_sourceless_compiled_packages": attrs.list(
            attrs.string(),
            default = [
                "androidx.databinding",
            ],
        ),
        "is_bootstrap_toolchain": attrs.bool(default = False),
        "jar": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "jar_builder": attrs.source(default = "prelude//toolchains/android/src/com/facebook/buck/util/zip:jar_builder"),
        "java": attrs.exec_dep(providers = [RunInfo]),
        "java_for_tests": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "javac": attrs.option(attrs.one_of(attrs.exec_dep(), attrs.source(), attrs.string()), default = None),
        "javac_protocol": attrs.enum(JavacProtocol.values()),
        "javacd": attrs.option(attrs.source(), default = None),
        "javacd_main_class": attrs.option(attrs.string(), default = None),
        "javacd_worker": attrs.option(attrs.dep(), default = None),
        "jlink": attrs.exec_dep(providers = [RunInfo]),
        "jmod": attrs.exec_dep(providers = [RunInfo]),
        "jrt_fs_jar": attrs.source(),
        "merge_class_to_source_maps": attrs.exec_dep(
            default = "prelude//java/tools:merge_class_to_source_maps",
            providers = [RunInfo],
        ),
        # `run_proguard` passes `-Xmx<value>`; same key and default as Meta's toolchain.
        "proguard_max_heap_size": attrs.string(default = read_root_config("tools", "proguard-max-heap-size", "1024M")),
        "source_level": attrs.string(default = "8"),
        "target_level": attrs.string(default = "8"),
        "track_class_usage": attrs.bool(default = False),
        "zip_scrubber": attrs.source(default = "prelude//toolchains/android/src/com/facebook/buck/util/zip:zip_scrubber"),
    },
)

def java_test_toolchain(name, **kwargs):
    kwargs["test_runner_library_jar"] = "prelude//toolchains/android/src/com/facebook/buck/testrunner:testrunner-bin-fixed"
    kwargs["junit_test_runner_main_class_args"] = ["com.facebook.buck.jvm.java.runner.FileClassPathRunner", "com.facebook.buck.testrunner.JUnitMain"]
    kwargs["junit5_test_runner_main_class_args"] = ["com.facebook.buck.jvm.java.runner.FileClassPathRunner", "com.facebook.buck.testrunner.JupiterMain"]
    kwargs["testng_test_runner_main_class_args"] = ["com.facebook.buck.jvm.java.runner.FileClassPathRunner", "com.facebook.buck.testrunner.TestNGMain"]
    kwargs["list_class_names"] = "prelude//java/tools:list_class_names"

    _java_test_toolchain_rule(name = name, **kwargs)

def _java_test_toolchain_rule_impl(ctx):
    return [
        DefaultInfo(),
        JavaTestToolchainInfo(
            junit5_test_runner_main_class_args = ctx.attrs.junit5_test_runner_main_class_args,
            junit_test_runner_main_class_args = ctx.attrs.junit_test_runner_main_class_args,
            jvm_args = ctx.attrs.jvm_args,
            list_class_names = ctx.attrs.list_class_names,
            test_runner_library_jar = ctx.attrs.test_runner_library_jar,
            testng_test_runner_main_class_args = ctx.attrs.testng_test_runner_main_class_args,
        ),
        TestListingInfo(
            list_tests = ctx.attrs.list_tests,
        ),
    ]

_java_test_toolchain_rule = rule(
    impl = _java_test_toolchain_rule_impl,
    attrs = {
        "junit5_test_runner_main_class_args": attrs.list(attrs.string()),
        "junit_test_runner_main_class_args": attrs.list(attrs.string()),
        "jvm_args": attrs.list(
            attrs.string(),
            default = [],
        ),
        "list_class_names": attrs.exec_dep(providers = [RunInfo]),
        "list_tests": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "test_runner_library_jar": attrs.source(),
        "testng_test_runner_main_class_args": attrs.list(attrs.string()),
    },
    is_toolchain_rule = True,
)
