# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:attrs_validators.bzl", "validation_common")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//:validation_deps.bzl", "VALIDATION_DEPS_ATTR_NAME", "get_validation_deps_outputs")
load(
    "@prelude//android:android_providers.bzl",
    "AndroidLibraryIntellijInfo",
    "AndroidResourceInfo",
    "merge_android_packageable_info",
    "merge_exported_android_resource_info",
)
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:build_only_native_code.bzl", "is_build_only_native_code")
load("@prelude//android:configuration.bzl", "is_building_android_binary_attr")
load("@prelude//android:r_dot_java.bzl", "get_dummy_r_dot_java")
load("@prelude//capabilities:capabilities_registration.bzl", "capabilities_registration_providers")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//graphql:graphql.bzl", "graphql_providers")
load("@prelude//java:java_library.bzl", "build_java_library")
load(
    "@prelude//java:java_providers.bzl",
    "JavaLibraryInfo",
    "JavaProviders",  # @unused Used as type
    "create_native_providers",
    "to_list",
)
load(
    "@prelude//java/utils:java_utils.bzl",
    "CustomJdkInfo",
    "get_java_version_attributes",
    "get_string_concat_inline_javac_args",
)
load("@prelude//jvm:attrs.bzl", "jvm_common", "min_sdk_version")
load("@prelude//kotlin:kotlin_library.bzl", "build_kotlin_library")
load("@prelude//target_stats:target_stats.bzl", "CycleMode", "target_stats_providers_and_subtargets")
load("@prelude//target_stats:target_stats_config.bzl", "TARGET_STATS_ENABLED")
load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:label_provider.bzl", "LabelInfo")
load(":attrs.bzl", "JvmLanguage", "android_common")

def get_custom_jdk_info(ctx: AnalysisContext) -> CustomJdkInfo:
    bootclasspath_entries = [] + ctx.attrs._android_toolchain[AndroidToolchainInfo].android_bootclasspath + optional_jars(ctx)
    bootclasspath_snapshots = [] + ctx.attrs._android_toolchain[AndroidToolchainInfo].android_bootclasspath_snapshots + optional_abi_jar_snapshots(ctx)

    return CustomJdkInfo(
        bootclasspath = bootclasspath_entries,
        bootclasspath_jar_snapshots = bootclasspath_snapshots,
        system_image = ctx.attrs._android_toolchain[AndroidToolchainInfo].jdk_system_image,
    )

def android_target_stats(ctx: AnalysisContext) -> (list[Provider], dict[str, list[Provider]]):
    if not TARGET_STATS_ENABLED:
        return [], {}
    tools = ctx.attrs._android_toolchain[AndroidToolchainInfo].target_stats_tools
    if tools == None:
        return [], {}
    return target_stats_providers_and_subtargets(
        ctx,
        tools = tools,
        srcs = {src.short_path: src for src in ctx.attrs.srcs},
        # Target stats cover the compile graph, including non-packaged provided deps.
        deps = (ctx.attrs.deps + ctx.attrs.exported_deps + ctx.attrs.runtime_deps + ctx.attrs.provided_deps + ctx.attrs.exported_provided_deps),
        cycle_mode = CycleMode("package"),
        module_name = ctx.label.name,
    )

def android_library_impl(ctx: AnalysisContext) -> list[Provider]:
    packaging_deps = ctx.attrs.deps + ctx.attrs.exported_deps + ctx.attrs.runtime_deps
    if ctx.attrs._build_only_native_code:
        shared_library_info, cxx_resource_info, linkable_graph = create_native_providers(ctx, ctx.label, packaging_deps)
        return [
            merge_android_packageable_info(ctx.label, ctx.actions, packaging_deps),
            shared_library_info,
            cxx_resource_info,
            linkable_graph,
            # Add an unused default output in case this target is used as an attr.source() anywhere.
            DefaultInfo(
                default_output = ctx.actions.write("{}/unused.jar".format(ctx.label.name), [], has_content_based_path = False),
                sub_targets = {
                    "generated_sources": [
                        DefaultInfo(default_output = ctx.actions.write("{}/generated_sources".format(ctx.label.name), [], has_content_based_path = False)),
                    ],
                },
            ),
            TemplatePlaceholderInfo(
                keyed_variables = {
                    "classpath": "unused_but_needed_for_analysis",
                },
            ),
        ]

    target_stats_providers, target_stats_subtargets = android_target_stats(ctx)

    java_providers, android_library_intellij_info = build_android_library(
        ctx = ctx,
        validation_deps_outputs = get_validation_deps_outputs(ctx),
        extra_sub_targets = target_stats_subtargets,
    )
    android_providers = [android_library_intellij_info] if android_library_intellij_info else []

    return (
        to_list(java_providers)
        + [
            merge_android_packageable_info(
                ctx.label,
                ctx.actions,
                packaging_deps,
                manifest = ctx.attrs.manifest,
            ),
            merge_exported_android_resource_info(ctx.attrs.exported_deps),
        ]
        + android_providers
        + [LabelInfo(labels = ctx.attrs.labels)]
        + graphql_providers(ctx)
        + capabilities_registration_providers(ctx)
        + target_stats_providers
    )

def optional_jars(ctx: AnalysisContext) -> list[Artifact]:
    if not ctx.attrs.android_optional_jars:
        return []

    result = []
    for dep in ctx.attrs.android_optional_jars:
        java_library_info = dep.get(JavaLibraryInfo)
        expect(
            java_library_info != None and java_library_info.library_output != None,
            "Only targets producing a Java bytecode output can be added as 'android_optional_jars'!",
        )
        result.append(java_library_info.library_output.full_library)

    return result

def optional_abi_jar_snapshots(ctx: AnalysisContext) -> list[Artifact]:
    if not ctx.attrs.android_optional_jars:
        return []

    result = []
    for dep in ctx.attrs.android_optional_jars:
        java_library_info = dep.get(JavaLibraryInfo)
        expect(
            java_library_info != None and java_library_info.library_output != None,
            "Only targets producing a Java bytecode output can be added as 'android_optional_jars'!",
        )

        if java_library_info.library_output.abi_jar_snapshot:
            result.append(java_library_info.library_output.abi_jar_snapshot)

    return result

def build_android_library(
    ctx: AnalysisContext,
    r_dot_java: JavaLibraryInfo | None = None,
    extra_sub_targets = {},
    validation_deps_outputs: [list[Artifact], None] = None,
) -> (JavaProviders, [AndroidLibraryIntellijInfo, None]):
    custom_jdk_info = get_custom_jdk_info(ctx)
    dummy_r_dot_java_info, android_library_intellij_info = _get_dummy_r_dot_java(ctx)
    extra_sub_targets = dict(extra_sub_targets)

    if r_dot_java:
        additional_classpath_entries = r_dot_java.compiling_deps
    elif dummy_r_dot_java_info:
        additional_classpath_entries = dummy_r_dot_java_info.compiling_deps
        extra_sub_targets["dummy_r_dot_java"] = [DefaultInfo(default_output = dummy_r_dot_java_info.library_output.full_library)]
    else:
        additional_classpath_entries = None

    source_level, _ = get_java_version_attributes(ctx)
    extra_arguments = get_string_concat_inline_javac_args(source_level)

    if ctx.attrs.patches_system_module:
        extra_arguments += [
            "--patch-module",
            "java.base=.",
        ]

    if ctx.attrs.language != None and ctx.attrs.language.lower() == "kotlin":
        return build_kotlin_library(
            ctx,
            additional_classpath_entries = additional_classpath_entries,
            custom_jdk_info = custom_jdk_info,
            extra_sub_targets = extra_sub_targets,
            validation_deps_outputs = validation_deps_outputs,
            extra_arguments = extra_arguments,
        ), android_library_intellij_info
    else:
        return build_java_library(
            ctx,
            ctx.attrs.srcs,
            additional_classpath_entries = additional_classpath_entries,
            custom_jdk_info = custom_jdk_info,
            extra_sub_targets = extra_sub_targets,
            validation_deps_outputs = validation_deps_outputs,
            extra_arguments = extra_arguments,
        ), android_library_intellij_info

def _get_dummy_r_dot_java(ctx: AnalysisContext) -> (JavaLibraryInfo | None, [AndroidLibraryIntellijInfo, None]):
    android_resources = dedupe([
        resource
        for resource in filter(
            None,
            [x.get(AndroidResourceInfo) for x in ctx.attrs.deps + ctx.attrs.provided_deps + (getattr(ctx.attrs, "provided_deps_query", []) or [])],
        )
        if resource.res != None
    ])
    if len(android_resources) == 0:
        return (None, None)

    dummy_r_dot_java_info = get_dummy_r_dot_java(
        ctx,
        ctx.attrs._android_toolchain[AndroidToolchainInfo].merge_android_resources[RunInfo],
        android_resources,
    )

    dummy_r_dot_java = dummy_r_dot_java_info.library_output
    return (
        dummy_r_dot_java_info,
        AndroidLibraryIntellijInfo(
            dummy_r_dot_java = dummy_r_dot_java.abi,
            android_resource_deps = android_resources,
        ),
    )

android_library = prelude_rule(
    name = "android_library",
    impl = android_library_impl,
    docs = """
        An `android_library()` rule is used to define a set of Java files
        that can be compiled together against the Android SDK. The main output of an
         `android_library()` rule is a single JAR file containing all of the
        compiled class files and resources.
    """,
    examples = """
        An `android_library` rule used in concert with an
         `android_resource()` rule.
        This would be a common arrangement for a standard Android Library project
        as defined by
         [http://developer.android.com/tools/projects/index.html](http://developer.android.com/tools/projects/index.html)

        ```
        android_resource(
          name = 'res',
          res = 'res',
          package = 'com.example',
        )

        android_library(
          name = 'my_library',
          srcs = glob(['src/**/*.java']),
          deps = [
            ':res',
          ],
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "srcs": attrs.list(
                attrs.source(),
                default = [],
                doc = """
                The set of `.java` files to compile for this rule.
            """,
            ),
            "resources": attrs.list(
                attrs.source(),
                default = [],
                doc = """
                Static files to include among the compiled `.class`
                 files. These files can be loaded via [Class.getResource()](http://docs.oracle.com/javase/7/docs/api/java/lang/Class.html#getResource(java.lang.String)).

                **Note:** Buck uses the `src_roots` property in
                 `.buckconfig`
                 to help determine where resources should be placed within the generated JAR file.
            """,
            ),
        }
        | android_common.manifest_arg()
        | {
            "deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                Rules (usually other `android_library` rules)
                 that are used to generate the classpath required to compile this
                 `android_library`.
            """,
            ),
            "extra_arguments": attrs.list(
                attrs.string(),
                default = [],
                doc = """
                List of additional arguments to pass into the Java compiler. These
                 arguments follow the ones specified in `.buckconfig`.
            """,
            ),
            "extra_kotlinc_arguments": attrs.list(
                attrs.arg(anon_target_compatible = True),
                default = [],
                doc = """
                List of additional arguments to pass into the Kotlin compiler.
            """,
            ),
            "source": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                Specifies the version of Java (as a string) to interpret source
                 files as.
                 Overrides the value in "source\\_level" in the "java" section
                 of `.buckconfig`.
            """,
            ),
            "target": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                Specifies the version of Java (as a string) for which to
                 generate code.
                 Overrides the value in "target\\_level" in the "java" section
                 of `.buckconfig`.
            """,
            ),
        }
        | jvm_common.annotation_processors()
        | jvm_common.exported_deps()
        | jvm_common.provided_deps()
        | jvm_common.exported_provided_deps()
        | buck.provided_deps_query_arg()
        | jvm_common.abi_generation_mode()
        | jvm_common.source_only_abi_deps()
        | jvm_common.required_for_source_only_abi()
        | jvm_common.kotlin_compiler_plugins()
        | jvm_common.incremental()
        | jvm_common.classic_java_content_based_paths()
        | android_common.patches_system_module_arg()
        | {
            "android_optional_jars": attrs.option(attrs.list(attrs.dep()), default = None),
            "capabilities_registrations": attrs.option(attrs.list(attrs.any()), default = None),
            "friend_paths": jvm_common.friend_paths_attr(),
            "jar_postprocessor": attrs.option(attrs.exec_dep(), default = None),
            "java_version": attrs.option(attrs.string(), default = None),
            "language": attrs.option(attrs.enum(JvmLanguage), default = None),
            "manifest_file": attrs.option(attrs.source(), default = None),
            "maven_coords": attrs.option(attrs.string(), default = None),
            "min_sdk_version": attrs.default_only(attrs.option(attrs.int(), default = min_sdk_version())),
            "proguard_config": attrs.option(attrs.source(), default = None),
            "remove_classes": attrs.list(
                attrs.regex(),
                default = [],
                doc = """
                List of classes to remove from the output jar. It only removes classes from the target's own
                 sources, not from any of its dependencies.
            """,
            ),
            "resources_root": attrs.option(attrs.string(), default = None),
            "runtime_deps": attrs.list(attrs.dep(), default = []),
            VALIDATION_DEPS_ATTR_NAME: attrs.set(attrs.dep(), sorted = True, default = []),
            "_android_toolchain": toolchains_common.android(),
            "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
            "_dex_toolchain": toolchains_common.dex(),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_is_building_android_binary": is_building_android_binary_attr(),
            "_java_toolchain": toolchains_common.java_for_android(),
            "_kotlin_toolchain": toolchains_common.kotlin_for_android(),
        }
    )
    | jvm_common.plugins()
    | validation_common.attrs_validators_arg()
    | validation_common.validation_specs_arg()
    | buck.licenses_arg()
    | buck.labels_arg()
    | buck.contacts_arg(),
)
