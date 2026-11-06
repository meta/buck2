# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//android:android_binary_native_library_rules.bzl", "get_android_binary_native_library_info")
load("@prelude//android:android_binary_resources_rules.bzl", "get_android_binary_resources_info")
load("@prelude//android:android_build_config.bzl", "generate_android_build_config", "get_build_config_fields", "write_build_config_defaults")
load(
    "@prelude//android:android_providers.bzl",
    "AndroidBinaryNativeLibsInfo",  # @unused Used as type
    "AndroidBinaryResourcesInfo",  # @unused Used as type
    "AndroidBuildConfigInfo",  # @unused Used as type
    "AndroidPreprocessedJavaClassesInfo",  # @unused Used as type
    "BuildConfigField",
    "DexFilesInfo",
    "merge_android_packageable_info",
)
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:compiler_depth.bzl", "compiler_depth_subtarget")
load("@prelude//android:configuration.bzl", "get_deps_by_platform")
load("@prelude//android:cpu_filters.bzl", "CPU_FILTER_FOR_DEFAULT_PLATFORM", "CPU_FILTER_FOR_PRIMARY_PLATFORM")
load("@prelude//android:dex_rules.bzl", "get_multi_dex", "get_single_primary_dex", "get_split_dex_merge_config", "merge_to_single_dex", "merge_to_split_dex")
load("@prelude//android:duplicate_class_check.bzl", "check_for_duplicate_classes_for_non_pre_dexed_jars", "check_for_duplicate_classes_for_pre_dexed_libs")
load("@prelude//android:exopackage.bzl", "get_exopackage_flags")
load("@prelude//android:preprocess_java_classes.bzl", "get_preprocessed_java_classes")
load("@prelude//android:util.bzl", "create_enhancement_context")
load("@prelude//android:voltron.bzl", "get_target_to_module_mapping")
load("@prelude//java:class_to_srcs.bzl", "JavaClassToSourceMapInfo", "merge_class_to_source_map_from_jar")
load(
    "@prelude//java:java_providers.bzl",
    "JavaPackagingDep",  # @unused Used as type
    "JavaPackagingInfo",
    "create_java_packaging_dep",
    "get_all_java_packaging_deps",
    "get_all_java_packaging_deps_from_packaging_infos",
)
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")
load("@prelude//java:proguard.bzl", "get_proguard_output")
load("@prelude//java/utils:java_utils.bzl", "get_class_to_source_map_info")
load("@prelude//utils:expect.bzl", "expect")

AndroidBinaryInfo = record(
    sub_targets = dict,
    class_to_srcs = JavaClassToSourceMapInfo,
    java_packaging_deps = list[JavaPackagingDep],
    deps_by_platform = dict,
    primary_platform = str,
    dex_files_info = DexFilesInfo,
    native_library_info = AndroidBinaryNativeLibsInfo,
    resources_info = AndroidBinaryResourcesInfo,
    materialized_artifacts = list[Artifact],
    preprocessed_java_classes_info = AndroidPreprocessedJavaClassesInfo | None,
    validation_outputs = list[Artifact],
)

def get_binary_info(ctx: AnalysisContext, use_proto_format: bool) -> AndroidBinaryInfo:
    sub_targets = {}
    materialized_artifacts = []
    preprocessed_java_classes_info = None

    deps_by_platform = get_deps_by_platform(ctx)
    primary_platform = CPU_FILTER_FOR_PRIMARY_PLATFORM if CPU_FILTER_FOR_PRIMARY_PLATFORM in deps_by_platform else CPU_FILTER_FOR_DEFAULT_PLATFORM
    deps = deps_by_platform[primary_platform]

    target_to_module_mapping_file = get_target_to_module_mapping(ctx, deps_by_platform)

    no_dx_target_labels = [no_dx_target.label.raw_target() for no_dx_target in ctx.attrs.no_dx]
    java_packaging_deps = [packaging_dep for packaging_dep in get_all_java_packaging_deps(ctx, deps)]

    android_packageable_info = merge_android_packageable_info(ctx.label, ctx.actions, deps)
    build_config_infos = list(android_packageable_info.build_config_infos.traverse()) if android_packageable_info.build_config_infos else []

    build_config_libs = get_build_config_java_libraries(ctx, build_config_infos, ctx.attrs.package_type, ctx.attrs.exopackage_modes)
    java_packaging_deps += get_all_java_packaging_deps_from_packaging_infos(ctx, build_config_libs)

    has_proguard_config = (
        ctx.attrs.proguard_config != None or ctx.attrs.android_sdk_proguard_config == "default" or ctx.attrs.android_sdk_proguard_config == "optimized"
    )
    should_pre_dex = not ctx.attrs.disable_pre_dex and not has_proguard_config and not ctx.attrs.preprocess_java_classes_bash

    enhancement_ctx = create_enhancement_context(ctx)
    if target_to_module_mapping_file:
        enhancement_ctx.debug_output("module.mapping", target_to_module_mapping_file)

    shared_libraries_to_exclude_set = set([label.raw_target() for label in ctx.attrs.shared_libraries_to_exclude])
    native_library_info = get_android_binary_native_library_info(
        enhancement_ctx,
        android_packageable_info,
        deps_by_platform,
        apk_module_graph_file = target_to_module_mapping_file,
        shared_libraries_to_exclude = shared_libraries_to_exclude_set,
        native_library_merge_glue = getattr(ctx.attrs, "native_library_merge_glue", None),
        native_library_merge_sequence = getattr(ctx.attrs, "native_library_merge_sequence", None),
        native_library_merge_map = getattr(ctx.attrs, "native_library_merge_map", None),
        native_library_merge_non_asset_libs = getattr(ctx.attrs, "native_library_merge_non_asset_libs", False),
        native_library_merge_linker_args = getattr(ctx.attrs, "native_library_merge_linker_args", None),
        native_library_merge_linker_args_all = getattr(ctx.attrs, "native_library_merge_linker_args_all", None),
        native_library_merge_code_generator = getattr(ctx.attrs, "native_library_merge_code_generator", None),
        native_library_merge_sequence_blocklist = getattr(ctx.attrs, "native_library_merge_sequence_blocklist", None),
    )
    java_packaging_deps.extend([
        create_java_packaging_dep(
            ctx,
            library_output,
        )
        for library_output in native_library_info.generated_java_code
    ])

    referenced_resources_lists = (
        [java_packaging_dep.dex.referenced_resources for java_packaging_dep in java_packaging_deps if java_packaging_dep.dex]
        if ctx.attrs.trim_resource_ids and should_pre_dex
        else []
    )
    resources_info = get_android_binary_resources_info(
        ctx,
        deps,
        android_packageable_info,
        java_packaging_deps,
        apk_module_graph_file = target_to_module_mapping_file,
        use_proto_format = use_proto_format,
        referenced_resources_lists = referenced_resources_lists,
        manifest_entries = ctx.attrs.manifest_entries,
        generate_strings_and_ids_separately = should_pre_dex,
        aapt2_preferred_density = ctx.attrs.aapt2_preferred_density,
    )
    sub_targets["manifest"] = [DefaultInfo(default_output = resources_info.manifest)]
    android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo]
    compiled_r_dot_java_deps = [
        create_java_packaging_dep(
            ctx,
            r_dot_java.library_info.library_output.full_library,
            dex_weight_factor = android_toolchain.r_dot_java_weight_factor,
            compiler_depth = r_dot_java.library_info.library_output.full_library_compiler_depth,
        )
        for r_dot_java in resources_info.r_dot_java_infos
    ]
    java_packaging_deps += compiled_r_dot_java_deps
    sub_targets["jvm_compiler_depth"] = compiler_depth_subtarget(ctx, java_packaging_deps)
    sub_targets["compiled_r_dot_java"] = [
        DefaultInfo(
            default_outputs = [compiled_r_dot_java_dep.jar for compiled_r_dot_java_dep in compiled_r_dot_java_deps],
        ),
    ]
    for r_dot_java_info in resources_info.r_dot_java_infos:
        sub_targets[r_dot_java_info.identifier + "_src"] = [
            DefaultInfo(
                default_output = r_dot_java_info.source_zipped,
            ),
        ]
    validation_outputs = list(native_library_info.validation_outputs)
    dex_java_packaging_deps = [
        packaging_dep for packaging_dep in java_packaging_deps if packaging_dep.dex and packaging_dep.dex.dex.owner.raw_target() not in no_dx_target_labels
    ]
    if should_pre_dex:
        pre_dexed_libs = [packaging_dep.dex for packaging_dep in dex_java_packaging_deps]
        if ctx.attrs.duplicate_class_checker_enabled:
            validation_outputs.append(
                check_for_duplicate_classes_for_pre_dexed_libs(
                    ctx,
                    {str(lib.class_names.owner.raw_target()): lib.class_names for lib in pre_dexed_libs if lib.dex},
                ),
            )
        if ctx.attrs.use_split_dex:
            dex_files_info = merge_to_split_dex(
                ctx,
                android_toolchain,
                pre_dexed_libs,
                get_split_dex_merge_config(ctx, android_toolchain),
                target_to_module_mapping_file,
                enable_bootstrap_dexes = ctx.attrs.enable_bootstrap_dexes,
            )
        else:
            dex_files_info = merge_to_single_dex(ctx, android_toolchain, pre_dexed_libs)
    else:
        jars_to_owners = {packaging_dep.jar: packaging_dep.jar.owner.raw_target() for packaging_dep in dex_java_packaging_deps}
        if ctx.attrs.duplicate_class_checker_enabled:
            validation_outputs.append(
                check_for_duplicate_classes_for_non_pre_dexed_jars(
                    ctx,
                    jars_to_owners,
                ),
            )

        if ctx.attrs.preprocess_java_classes_bash:
            jars_to_owners, preprocessed_java_classes_info = get_preprocessed_java_classes(enhancement_ctx, jars_to_owners)
            if preprocessed_java_classes_info:
                materialized_artifacts.append(preprocessed_java_classes_info.materialized_artifacts_dir)
        if has_proguard_config:
            additional_proguard_configs = (
                [resources_info.proguard_config_file] if not ctx.attrs.ignore_aapt_proguard_config and resources_info.proguard_config_file else []
            )

            no_dx_for_additional_jars = [no_dx for no_dx in ctx.attrs.no_dx if no_dx not in ctx.attrs.exclude_duplicate_targets_do_not_use]
            additional_jars = (
                android_toolchain.android_bootclasspath
                + android_toolchain.android_optional_jars
                + [no_dx[DefaultInfo].default_outputs[0] for no_dx in no_dx_for_additional_jars if len(no_dx[DefaultInfo].default_outputs) == 1]
            )

            proguard_output = get_proguard_output(
                ctx,
                jars_to_owners,
                java_packaging_deps,
                additional_proguard_configs,
                additional_jars,
                ctx.attrs.android_sdk_proguard_config,
                android_toolchain.proguard_config,
                android_toolchain.optimized_proguard_config,
                android_toolchain.proguard_jar,
                ctx.attrs.skip_proguard,
            )
            materialized_artifacts.extend(proguard_output.proguard_artifacts)
            jars_to_owners = proguard_output.jars_to_owners
            dir_srcs = {artifact.basename: artifact for artifact in proguard_output.proguard_artifacts}
            for i, hidden_artifact in enumerate(proguard_output.proguard_hidden_artifacts):
                dir_srcs["hidden/{}_{}".format(i, hidden_artifact.basename)] = hidden_artifact
            sub_targets["proguard_text_output"] = [
                DefaultInfo(
                    default_output = ctx.actions.symlinked_dir(
                        "proguard_text_output",
                        dir_srcs,
                        has_content_based_path = False,
                    ),
                ),
            ]
        else:
            proguard_output = None

        if ctx.attrs.use_split_dex:
            dex_files_info = get_multi_dex(
                ctx,
                android_toolchain,
                jars_to_owners,
                ctx.attrs.primary_dex_patterns,
                proguard_output.proguard_configuration_output_file if proguard_output else None,
                proguard_output.proguard_mapping_output_file if proguard_output else None,
                is_optimized = has_proguard_config,
                apk_module_graph_file = target_to_module_mapping_file,
                enable_bootstrap_dexes = ctx.attrs.enable_bootstrap_dexes,
                multidex_min_api = ctx.attrs.multidex_min_api,
            )
        else:
            dex_files_info = get_single_primary_dex(
                ctx,
                android_toolchain,
                jars_to_owners.keys(),
                is_optimized = has_proguard_config,
            )

    sub_targets = sub_targets | enhancement_ctx.get_sub_targets()
    if resources_info.string_source_map:
        sub_targets["generate_string_resources"] = [DefaultInfo(default_output = resources_info.string_source_map)]

    if resources_info.voltron_string_source_map:
        sub_targets["generate_voltron_string_resources"] = [DefaultInfo(default_output = resources_info.voltron_string_source_map)]

    if dex_files_info.primary_dex_class_names:
        sub_targets["primary_dex_class_names"] = [DefaultInfo(default_output = dex_files_info.primary_dex_class_names)]

    class_to_srcs, _, _ = get_class_to_source_map_info(ctx, outputs = None, deps = deps)
    transitive_class_to_src_map = merge_class_to_source_map_from_jar(
        actions = ctx.actions,
        name = ctx.label.name + ".transitive_class_to_src.json",
        java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo],
        relative_to = None,
        deps = [class_to_srcs],
    )
    sub_targets["transitive_class_to_src_map"] = [DefaultInfo(default_output = transitive_class_to_src_map)]

    return AndroidBinaryInfo(
        sub_targets = sub_targets,
        class_to_srcs = class_to_srcs,
        java_packaging_deps = java_packaging_deps,
        deps_by_platform = deps_by_platform,
        primary_platform = primary_platform,
        dex_files_info = dex_files_info,
        native_library_info = native_library_info,
        resources_info = resources_info,
        materialized_artifacts = materialized_artifacts,
        preprocessed_java_classes_info = preprocessed_java_classes_info,
        validation_outputs = validation_outputs,
    )

def get_build_config_java_libraries(
    ctx: AnalysisContext,
    build_config_infos: list[AndroidBuildConfigInfo],
    package_type: str,
    exopackage_modes: list[str],
    is_binary: bool = True,
) -> list[JavaPackagingInfo]:
    # BuildConfig deps should not be added for instrumented APKs because BuildConfig.class has
    # already been added to the APK under test.
    if package_type == "instrumented":
        return []

    build_config_constants = [
        BuildConfigField(type = "boolean", name = "DEBUG", value = str(package_type != "release").lower()),
        BuildConfigField(type = "boolean", name = "IS_EXOPACKAGE", value = str(len(exopackage_modes) > 0).lower()),
        BuildConfigField(type = "int", name = "EXOPACKAGE_FLAGS", value = str(get_exopackage_flags(exopackage_modes))),
    ]

    binary_build_config_values = {field.name: field for field in get_build_config_fields(ctx.attrs.build_config_values) + build_config_constants}
    binary_inline_values_file = ctx.actions.write(
        "binary_build_config_values",
        ["{} {} = {}".format(field.type, field.name, field.value) for field in binary_build_config_values.values()],
        has_content_based_path = True,
    )

    android_binary_values_file = (
        ctx.attrs.build_config_values_file[DefaultInfo].default_outputs[0]
        if isinstance(ctx.attrs.build_config_values_file, Dependency)
        else ctx.attrs.build_config_values_file
    )

    java_libraries = []
    java_packages_seen = []
    for build_config_info in build_config_infos:
        java_package = build_config_info.package
        expect(java_package not in java_packages_seen, "Got the same java_package {} for different AndroidBuildConfigs".format(java_package))
        java_packages_seen.append(java_package)

        default_values = {field.name: field for field in build_config_info.build_config_fields + build_config_constants}
        default_values_files = [write_build_config_defaults(ctx, java_package, default_values.values())]
        if build_config_info.values_file:
            default_values_files.append(build_config_info.values_file)

        values_files = [binary_inline_values_file]
        if android_binary_values_file:
            values_files.append(android_binary_values_file)
        if not is_binary:
            default_values_files.extend(values_files)
            values_files = []
        java_libraries.append(
            generate_android_build_config(
                ctx,
                java_package,
                java_package,
                use_constant_expressions = True,
                default_values_files = default_values_files,
                values_files = values_files,
            )[1]
        )

    return java_libraries
