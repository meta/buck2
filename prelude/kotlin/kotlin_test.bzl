# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:attrs_validators.bzl", "validation_common")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//android:build_only_native_code.bzl", "is_build_only_native_code")
load("@prelude//decls:common.bzl", "TestType", "buck")
load("@prelude//decls:re_test_common.bzl", "re_test_common")
load("@prelude//decls:test_common.bzl", "test_common")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//java:java_library.bzl", "jvm_target_stats")
load("@prelude//java:java_test.bzl", "build_junit_test")
load("@prelude//jvm:attrs.bzl", "jvm_common")
load("@prelude//kotlin:kotlin_library.bzl", "build_kotlin_library")
load("@prelude//test:inject_test_run_info.bzl", "inject_test_run_info")
load(":attrs.bzl", "no_x_jdk_release_arg")

def kotlin_test_impl(ctx: AnalysisContext) -> list[Provider]:
    if ctx.attrs._build_only_native_code:
        return [DefaultInfo()]

    target_stats_providers, target_stats_subtargets = jvm_target_stats(ctx)
    java_providers = build_kotlin_library(
        ctx,
        extra_sub_targets = target_stats_subtargets,
    )
    external_runner_test_info = build_junit_test(ctx, java_providers.java_library_info, java_providers.java_packaging_info, java_providers.class_to_src_map)

    return (
        inject_test_run_info(ctx, external_runner_test_info)
        + [
            java_providers.java_library_intellij_info,
            java_providers.java_library_info,
            java_providers.java_packaging_info,
            java_providers.template_placeholder_info,
            java_providers.default_info,
            java_providers.class_to_src_map,
        ]
        + target_stats_providers
        + ([java_providers.validation_info] if java_providers.validation_info else [])
    )

kotlin_test = prelude_rule(
    name = "kotlin_test",
    impl = kotlin_test_impl,
    docs = """
        A `kotlin_test()` rule is used to define a set of
         `.kt` files that contain tests to run via JUnit.
    """,
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        buck.inject_test_env_arg()
        | {
            "resources": attrs.list(
                attrs.source(),
                default = [],
                doc = """
                Same as `kotlin_library()`.
            """,
            ),
            "srcs": attrs.list(
                attrs.source(),
                default = [],
                doc = """
                Like ``kotlin_library()``,
                 all of the `.kt` files specified by the
                 `srcs` argument will be compiled when this rule is
                 built. In addition, all of the corresponding `.class` files that are built by this rule will be passed as arguments to
                 JUnit when this rule is run as a test. `.class` files
                 that are passed to JUnit that do not have any methods annotated with
                 `@Test` are considered failed tests, so make sure that
                 only test case classes are specified as `srcs`. This is
                 frequently done by specifying `srcs` as
                 `glob(['**/*Test.kt'])`.
            """,
            ),
        }
        | buck.test_label_arg()
        | {
            "deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                Same as `kotlin_library()`.
                 // org.junit.rules.Timeout was not introduced until 4.7.
                 Must include JUnit (version 4.7 or later) as a dependency for JUnit tests.
                 Must include TestNG (version 6.2 or later) and hamcrest as a dependencies for TestNG tests.
            """,
            ),
            "test_type": attrs.option(
                attrs.enum(TestType),
                default = None,
                doc = """
                Specifies which test framework to use.
                 The currently supported options are 'junit' and 'testng'.
            """,
            ),
        }
        | buck.run_test_separately_arg(run_test_separately_type = attrs.bool(default = False))
        | re_test_common.test_args()
        | buck.test_rule_timeout_ms()
        | {
            "vm_args": attrs.list(
                attrs.arg(),
                default = [],
                doc = """
                Runtime arguments to the JVM running the tests.
            """,
            ),
        }
        | jvm_common.annotation_processors()
        | jvm_common.incremental()
        | jvm_common.classic_java_content_based_paths()
        | jvm_common.javac()
        | jvm_common.kotlin_compiler_plugins()
        | jvm_common.plugins()
        | jvm_common.test_env()
        | jvm_common.abi_generation_mode()
        | {
            "cxx_library_allowlist": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                List of cxx_library targets to build, if use_cxx_libraries is true.
                This can be useful if some dependencies are Android-only and won't build for the test host platform.
            """,
            ),
            "deps_query": attrs.option(attrs.query(), default = None),
            "discover_all_test_classes": attrs.bool(default = False),
            "exported_deps": attrs.list(attrs.dep(), default = []),
            "exported_provided_deps": attrs.list(attrs.dep(), default = []),
            "extra_arguments": attrs.list(attrs.string(), default = []),
            "extra_kotlinc_arguments": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
            "friend_paths": jvm_common.friend_paths_attr(),
            "java": attrs.option(attrs.dep(), default = None),
            "java_agents": attrs.list(attrs.source(), default = []),
            "java_version": attrs.option(attrs.string(), default = None),
            "jdk_release": attrs.option(attrs.string(), default = None),
            "manifest_file": attrs.option(attrs.source(), default = None),
            "maven_coords": attrs.option(attrs.string(), default = None),
            "proguard_config": attrs.option(attrs.source(), default = None),
            "provided_deps": attrs.list(attrs.dep(), default = []),
            "remove_classes": attrs.list(attrs.regex(), default = []),
            "required_for_source_only_abi": attrs.bool(default = False),
            "resources_root": attrs.option(attrs.string(), default = None),
            "runtime_deps": attrs.list(attrs.dep(), default = []),
            "source": attrs.option(attrs.string(), default = None),
            "source_only_abi_deps": attrs.list(attrs.dep(), default = []),
            "supports_test_execution_caching": attrs.bool(default = True),
            "target": attrs.option(attrs.string(), default = None),
            "test_case_timeout_ms": attrs.option(attrs.int(), default = None),
            "test_class_names_file": attrs.option(attrs.source(), default = None),
            "test_info_type": attrs.option(attrs.string(), default = None),
            "unbundled_resources_root": attrs.option(attrs.source(allow_directory = True), default = None),
            "use_cxx_libraries": attrs.option(attrs.bool(), default = None),
            "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_is_building_android_binary": attrs.default_only(attrs.bool(default = False)),
            "_java_test_toolchain": toolchains_common.java_test(),
            "_java_toolchain": toolchains_common.java(),
            "_kotlin_toolchain": toolchains_common.kotlin(),
        }
        | no_x_jdk_release_arg()
        | buck.licenses_arg()
        | buck.contacts_arg()
        | validation_common.attrs_validators_arg()
        | test_common.attributes()
    ),
)
