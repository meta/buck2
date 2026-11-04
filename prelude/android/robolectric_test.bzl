# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:attrs_validators.bzl", "validation_common")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//android:android_binary_resources_rules.bzl", "get_android_binary_resources_info")
load("@prelude//android:android_library.bzl", "android_target_stats", "build_android_library", "optional_jars")
load("@prelude//android:android_providers.bzl", "merge_android_packageable_info")
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:build_only_native_code.bzl", "is_build_only_native_code")
load("@prelude//android:resource_source_map.bzl", "create_resource_source_map")
load("@prelude//decls:common.bzl", "TestType", "buck")
load("@prelude//decls:re_test_common.bzl", "re_test_common")
load("@prelude//decls:test_common.bzl", "test_common")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//java:java_providers.bzl", "JavaLibraryInfo")
load("@prelude//java:java_test.bzl", "build_junit_test")
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")
load("@prelude//jvm:attrs.bzl", "jvm_common")
load("@prelude//test:inject_test_run_info.bzl", "inject_test_run_info")
load("@prelude//utils:expect.bzl", "expect")
load(":attrs.bzl", "JvmLanguage", "android_common")

def robolectric_test_impl(ctx: AnalysisContext) -> list[Provider]:
    if ctx.attrs._build_only_native_code:
        return [DefaultInfo()]

    extra_cmds = []

    # Force robolectric to only use local dependency resolution.
    extra_cmds.append("-Drobolectric.offline=true")
    if ctx.attrs.robolectric_runtime_dependency:
        runtime_dependencies_dir = ctx.attrs.robolectric_runtime_dependency
    elif ctx.attrs.robolectric_runtime_dependencies:
        runtime_dependencies_dir = ctx.actions.symlinked_dir(
            "runtime_dependencies",
            {runtime_dep.basename: runtime_dep for runtime_dep in ctx.attrs.robolectric_runtime_dependencies},
            has_content_based_path = False,
        )
    else:
        runtime_dependencies_dir = None

    if runtime_dependencies_dir:
        extra_cmds.append(cmd_args(runtime_dependencies_dir, format = "-Drobolectric.dependency.dir={}"))

    all_packaging_deps = ctx.attrs.deps + ctx.attrs.exported_deps + ctx.attrs.runtime_deps
    android_packageable_info = merge_android_packageable_info(ctx.label, ctx.actions, all_packaging_deps)
    resources_info = get_android_binary_resources_info(
        ctx,
        all_packaging_deps,
        android_packageable_info,
        java_packaging_deps = [],  # Only used for third-party jar resources, which we don't care about here.
        use_proto_format = False,
        referenced_resources_lists = [],
        generate_strings_and_ids_separately = False,
        aapt2_preferred_density = ctx.attrs.aapt2_preferred_density,
        manifest_entries = ctx.attrs.manifest_entries,
    )

    resource_source_map = None
    resource_source_map_jar = ctx.attrs.env.get("ROBOLECTRIC_RESOURCE_SOURCE_MAP_JAR")
    if resource_source_map_jar:
        resource_source_map = create_resource_source_map(
            ctx,
            resources_info.unfiltered_resource_infos,
            resource_source_map_jar,
            category = "robolectric_resource_source_map",
        )

    test_config_properties_file = ctx.actions.write(
        "test_config.properties",
        [
            # Replace \ with \\ for Windows compatibility
            cmd_args(["android_resource_apk", resources_info.primary_resources_apk], delimiter = "=", replace_regex = ("\\\\\\b", "\\\\")),
            cmd_args(["android_merged_manifest", resources_info.manifest], delimiter = "=", replace_regex = ("\\\\\\b", "\\\\")),
        ]
        + (
            [cmd_args(["robolectric_resource_source_map", resource_source_map], delimiter = "=", replace_regex = ("\\\\\\b", "\\\\"))]
            if resource_source_map
            else []
        ),
        has_content_based_path = False,
    )

    # Robolectric looks for a file named /com/android/tools/test_config.properties on the classpath
    test_config_symlinked_dir = ctx.actions.symlinked_dir(
        "test_config_symlinked_dir", {"com/android/tools/test_config.properties": test_config_properties_file}, has_content_based_path = False
    )
    test_config_properties_jar = ctx.actions.declare_output("test_config_properties.jar", has_content_based_path = False)
    jar_cmd = cmd_args([
        ctx.attrs._java_toolchain[JavaToolchainInfo].jar,
        "-cfM",  # -c: create new archive, -f: specify the file name, -M: do not create a manifest
        test_config_properties_jar.as_output(),
        "-C",
        test_config_symlinked_dir,
        ".",
    ])
    ctx.actions.run(jar_cmd, category = "test_config_properties_jar_cmd")
    extra_cmds.append(cmd_args(hidden = [resources_info.primary_resources_apk, resources_info.manifest] + ([resource_source_map] if resource_source_map else [])))

    r_dot_javas = [r_dot_java.library_info for r_dot_java in resources_info.r_dot_java_infos if r_dot_java.library_info.library_output]
    expect(len(r_dot_javas) <= 1, "android_library only works with single R.java")

    target_stats_providers, extra_sub_targets = android_target_stats(ctx)
    if resource_source_map:
        extra_sub_targets["resource_source_map"] = [DefaultInfo(default_output = resource_source_map)]
    if r_dot_javas:
        r_dot_java = r_dot_javas[0]
        extra_sub_targets["r_dot_java"] = [DefaultInfo(default_output = r_dot_java.library_output.full_library)]
    else:
        r_dot_java = None
    java_providers, _ = build_android_library(ctx, r_dot_java = r_dot_java, extra_sub_targets = extra_sub_targets)

    extra_classpath_entries = [test_config_properties_jar] + ctx.attrs._android_toolchain[AndroidToolchainInfo].android_bootclasspath + optional_jars(ctx)
    extra_classpath_entries.extend([r_dot_java.library_output.full_library for r_dot_java in r_dot_javas])
    external_runner_test_info = build_junit_test(
        ctx,
        java_providers.java_library_info,
        java_providers.java_packaging_info,
        java_providers.class_to_src_map,
        extra_cmds = extra_cmds,
        extra_classpath_entries = extra_classpath_entries,
    )

    providers = inject_test_run_info(ctx, external_runner_test_info) + [
        java_providers.java_library_intellij_info,
        java_providers.java_packaging_info,
        java_providers.template_placeholder_info,
        java_providers.default_info,
        java_providers.class_to_src_map,
        java_providers.java_global_code_info,
    ]

    java_library_without_compiling_deps = JavaLibraryInfo(
        compiling_deps = None,
        library_output = java_providers.java_library_info.library_output,
        output_for_classpath_macro = java_providers.java_library_info.output_for_classpath_macro,
    )
    providers.append(java_library_without_compiling_deps)
    providers.extend(target_stats_providers)
    if java_providers.validation_info:
        providers.append(java_providers.validation_info)

    return providers

robolectric_test = prelude_rule(
    name = "robolectric_test",
    impl = robolectric_test_impl,
    docs = """
        A `robolectric_test()` rule is used to define a set of
         `.java` files that contain tests to run via JUnit
        with Robolectric test runner. It extends from `java_test()` rule.
    """,
    examples = None,
    further = None,
    attrs = (
        # @unsorted-dict-items
        buck.inject_test_env_arg()
        | {
            "aapt2_preferred_density": attrs.option(attrs.string(), default = None),
            "android_optional_jars": attrs.option(attrs.list(attrs.dep()), default = None),
            "compiled_resource_apks": attrs.list(attrs.source(), default = []),
            "cxx_library_allowlist": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                List of cxx_library targets to build, if use_cxx_libraries is true.
                This can be useful if some dependencies are Android-only and won't build for the test host platform.
            """,
            ),
            "deps": attrs.list(attrs.dep(), default = []),
            "discover_all_test_classes": attrs.bool(default = False),
            "env": attrs.dict(key = attrs.string(), value = attrs.arg(), sorted = False, default = {}),
            "exported_deps": attrs.list(attrs.dep(), default = []),
            "exported_provided_deps": attrs.list(attrs.dep(), default = []),
            "extra_arguments": attrs.list(attrs.string(), default = []),
            "extra_kotlinc_arguments": attrs.list(
                attrs.arg(anon_target_compatible = True),
                default = [],
                doc = """
                List of additional arguments to pass into the Kotlin compiler.
            """,
            ),
            "friend_paths": jvm_common.friend_paths_attr(),
            "jar_postprocessor": attrs.option(attrs.exec_dep(), default = None),
            "java": attrs.option(attrs.dep(), default = None),
            "java_agents": attrs.list(attrs.source(), default = []),
            "java_version": attrs.option(attrs.string(), default = None),
            "language": attrs.option(attrs.enum(JvmLanguage), default = None),
            "locales_for_binary_resources": attrs.list(attrs.string(), default = []),
            "manifest_entries": attrs.dict(key = attrs.string(), value = attrs.any(), default = {}),
            "manifest_file": attrs.option(attrs.source(), default = None),
            "manifest_skeleton": attrs.option(attrs.source(), default = None),
            "maven_coords": attrs.option(attrs.string(), default = None),
            "proguard_config": attrs.option(attrs.source(), default = None),
            "provided_deps": attrs.list(attrs.dep(), default = []),
            "provided_deps_query": attrs.option(attrs.query(), default = None),
            "remove_classes": attrs.list(attrs.regex(), default = []),
            "required_for_source_only_abi": attrs.bool(default = False),
            "resource_stable_ids": attrs.option(attrs.source(), default = None),
            "resources": attrs.list(attrs.source(), default = []),
            "resources_root": attrs.option(attrs.string(), default = None),
            "robolectric_runtime_dependencies": attrs.list(attrs.source(), default = []),
            "robolectric_runtime_dependency": attrs.option(
                attrs.source(),
                default = None,
                doc = """
                Robolectric only runs in offline mode with buck. Specify the relative
                 directory containing all the jars Robolectric uses at runtime.
            """,
            ),
            "run_test_separately": attrs.bool(default = False),
            "runtime_deps": attrs.list(attrs.dep(), default = []),
            "source": attrs.option(attrs.string(), default = None),
            "source_only_abi_deps": attrs.list(attrs.dep(), default = []),
            "srcs": attrs.list(attrs.source(), default = []),
            "supports_test_execution_caching": attrs.bool(default = True),
            "target": attrs.option(attrs.string(), default = None),
            "test_case_timeout_ms": attrs.option(attrs.int(), default = None),
            "test_class_names_file": attrs.option(attrs.source(), default = None),
            "test_rule_timeout_ms": attrs.option(attrs.int(), default = None),
            "test_type": attrs.option(attrs.enum(TestType), default = None),
            "unbundled_resources_root": attrs.option(attrs.source(allow_directory = True), default = None),
            "use_cxx_libraries": attrs.option(attrs.bool(), default = None),
            "vm_args": attrs.list(attrs.arg(), default = []),
            "_android_toolchain": toolchains_common.android(),
            "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_is_building_android_binary": attrs.default_only(attrs.bool(default = False)),
            "_java_test_toolchain": toolchains_common.java_test(),
            "_java_toolchain": toolchains_common.java_for_host_test(),
            "_kotlin_toolchain": toolchains_common.kotlin(),
        }
        | android_common.manifest_arg()
        | android_common.patches_system_module_arg()
        | jvm_common.abi_generation_mode()
        | jvm_common.annotation_processors()
        | jvm_common.incremental()
        | jvm_common.plugins()
        | jvm_common.kotlin_compiler_plugins()
        | jvm_common.classic_java_content_based_paths()
        | re_test_common.test_args()
        | test_common.attributes()
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | validation_common.attrs_validators_arg()
    ),
)
