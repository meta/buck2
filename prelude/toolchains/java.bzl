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
    "JavaPlatformInfo",
    "JavaTestToolchainInfo",
    "JavaToolchainInfo",
    "JavacProtocol",
    "PrebuiltJarToolchainInfo",
)
load("@prelude//tests:test_listing.bzl", "TestListingInfo")

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
    output = ctx.actions.declare_output(paths.basename(ctx.attrs.jar), has_content_based_path = False)
    ctx.actions.run(cmd_args(["ln", "-s", ctx.attrs.jar, output.as_output()]), category = "{}_symlink".format(ctx.attrs.name))
    return [DefaultInfo(default_output = output)]

system_java_lib = rule(
    impl = _system_java_lib_impl,
    attrs = {
        "jar": attrs.string(),
    },
)

def system_prebuilt_jar_bootstrap_toolchain(name, java, visibility = None):
    kwargs = {}

    _prebuilt_jar_toolchain_rule(name = name, java = java, visibility = visibility, **kwargs)

def _prebuilt_jar_toolchain_rule_impl(ctx):
    return [
        DefaultInfo(),
        PrebuiltJarToolchainInfo(
            class_abi_generator = None,
            cp_snapshot_generator = None,
            global_code_config = {},
            is_bootstrap_toolchain = True,
            java = ctx.attrs.java,
        ),
    ]

_prebuilt_jar_toolchain_rule = rule(
    attrs = {
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
        class_abi_generator = "prelude//toolchains/android/src/com/facebook/buck/jvm/java/abi:api-stubber",
        class_loader_bootstrapper = "prelude//toolchains/android/src/com/facebook/buck/cli/bootstrapper:bootstrapper",
        fat_jar_main_class_lib = "prelude//toolchains/android/src/com/facebook/buck/jvm/java/fatjar:fat-jar-main-binary",
        javac = javac,
        javacd = "prelude//toolchains/android/src/com/facebook/buck/jvm/java/stepsbuilder/javacd/main:javacd_tool",
        javac_protocol = "javacd",
        javacd_main_class = "com.facebook.buck.jvm.java.stepsbuilder.javacd.main.JavaCDMain",
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
            # TODO(navidq) make this configurable via buck config
            abi_generation_mode = AbiGenerationMode("none"),
            compile_and_package = ctx.attrs.compile_and_package,
            class_abi_generator = ctx.attrs.class_abi_generator,
            class_loader_bootstrapper = ctx.attrs.class_loader_bootstrapper,
            cp_snapshot_generator = None,
            dep_files = None,
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
            jar_builder = RunInfo(cmd_args([ctx.attrs.java[RunInfo], "-jar", ctx.attrs.jar_builder])),
            jlink = ctx.attrs.jlink,
            jmod = ctx.attrs.jmod,
            jrt_fs_jar = ctx.attrs.jrt_fs_jar,
            src_root_elements = [],
            src_root_prefixes = [],
            track_class_usage = False,
            zip_scrubber = RunInfo(cmd_args([ctx.attrs.java[RunInfo], "-jar", ctx.attrs.zip_scrubber])),
            nullsafe = None,
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
        "jlink": attrs.exec_dep(providers = [RunInfo]),
        "jmod": attrs.exec_dep(providers = [RunInfo]),
        "jrt_fs_jar": attrs.source(),
        "merge_class_to_source_maps": attrs.exec_dep(
            default = "prelude//java/tools:merge_class_to_source_maps",
            providers = [RunInfo],
        ),
        "source_level": attrs.string(default = "8"),
        "target_level": attrs.string(default = "8"),
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
