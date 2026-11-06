# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//android:android_binary.bzl", "get_build_config_java_libraries")
load("@prelude//android:android_binary_native_library_rules.bzl", "get_android_binary_native_library_info")
load("@prelude//android:android_binary_resources_rules.bzl", "get_cxx_resources", "get_manifest")
load(
    "@prelude//android:android_providers.bzl",
    "AndroidResourceInfo",
    "ExportedAndroidResourceInfo",
    "get_all_android_packageable_targets",
    "merge_android_packageable_info",
)
load("@prelude//android:android_resource.bzl", "get_text_symbols")
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:configuration.bzl", "CPU_TRANSITION_ATTRS", "cpu_split_transition", "get_deps_by_platform")
load("@prelude//android:cpu_filters.bzl", "ALL_CPU_FILTERS", "CPU_FILTER_FOR_DEFAULT_PLATFORM", "CPU_FILTER_FOR_PRIMARY_PLATFORM")
load("@prelude//android:util.bzl", "create_enhancement_context")
load("@prelude//decls:common.bzl", "TargetCpuType", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//java:java_providers.bzl", "create_java_packaging_dep", "get_all_java_packaging_deps", "get_all_java_packaging_deps_from_packaging_infos")
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")
load("@prelude//jvm:attrs.bzl", "jvm_common")
load("@prelude//target_stats:target_stats.bzl", "target_stats_aggregate_providers_and_subtargets")
load("@prelude//utils:argfile.bzl", "argfile")
load("@prelude//utils:utils.bzl", "flatten")
load(":attrs.bzl", "DISABLE_STRIPPING", "GatoradePhase", "JvmLanguage")

def android_aar_impl(ctx: AnalysisContext) -> list[Provider]:
    deps_by_platform = get_deps_by_platform(ctx)
    primary_platform = CPU_FILTER_FOR_PRIMARY_PLATFORM if CPU_FILTER_FOR_PRIMARY_PLATFORM in deps_by_platform else CPU_FILTER_FOR_DEFAULT_PLATFORM
    deps = deps_by_platform[primary_platform]

    excluded_java_packaging_deps = get_all_java_packaging_deps(ctx, ctx.attrs.excluded_java_deps)
    excluded_java_packaging_deps_targets = set([excluded_dep.label.raw_target() for excluded_dep in excluded_java_packaging_deps])
    java_packaging_deps = [
        packaging_dep
        for packaging_dep in get_all_java_packaging_deps(ctx, deps)
        if packaging_dep.label.raw_target() not in excluded_java_packaging_deps_targets
    ]
    android_packageable_info = merge_android_packageable_info(ctx.label, ctx.actions, deps)

    excluded_android_packageable_targets = set(get_all_android_packageable_targets(ctx.attrs.excluded_java_deps))
    manifest_infos = android_packageable_info.manifests.traverse(ordering = "topological") if android_packageable_info.manifests else []
    manifests = [manifest_info.manifest for manifest_info in manifest_infos if manifest_info.target_label not in excluded_android_packageable_targets]
    android_manifest = get_manifest(ctx, manifests, ctx.attrs.manifest_entries, should_replace_application_id_placeholders = False)

    if ctx.attrs.include_build_config_class:
        build_config_infos = list(android_packageable_info.build_config_infos.traverse()) if android_packageable_info.build_config_infos else []
        java_packaging_deps.extend(
            get_all_java_packaging_deps_from_packaging_infos(
                ctx,
                get_build_config_java_libraries(ctx, build_config_infos, package_type = "release", exopackage_modes = [], is_binary = False),
            )
        )

    enhancement_ctx = create_enhancement_context(ctx)
    android_binary_native_library_info = get_android_binary_native_library_info(
        enhancement_ctx,
        android_packageable_info,
        deps_by_platform,
        prebuilt_native_library_dirs_to_exclude = excluded_android_packageable_targets,
        shared_libraries_to_exclude = excluded_android_packageable_targets,
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
        for library_output in android_binary_native_library_info.generated_java_code
    ])

    jars = [dep.jar for dep in java_packaging_deps if dep.jar]
    classes_jar = ctx.actions.declare_output("classes.jar", has_content_based_path = False)
    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    classes_jar_cmd = cmd_args([
        java_toolchain.jar_builder,
        "--entries-to-jar",
        argfile(actions = ctx.actions, name = "classes_jar_entries.txt", args = jars),
        "--output",
        classes_jar.as_output(),
    ])

    if ctx.attrs.remove_classes:
        remove_classes_file = ctx.actions.write("remove_classes.txt", ctx.attrs.remove_classes, has_content_based_path = False)
        classes_jar_cmd.add([
            "--blocklist-patterns",
            remove_classes_file,
            "--blocklist-patterns-matcher",
            "remove_classes_patterns_matcher",
        ])

    ctx.actions.run(classes_jar_cmd, category = "create_classes_jar")

    sub_targets = {}
    dependency_sources_jars = [dep.sources_jar for dep in java_packaging_deps if dep.sources_jar]
    if dependency_sources_jars:
        combined_sources_jar = ctx.actions.declare_output("sources.jar", has_content_based_path = False)
        java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
        combined_sources_jar_cmd = cmd_args([
            java_toolchain.jar_builder,
            "--entries-to-jar",
            argfile(actions = ctx.actions, name = "combined_sources_jar_entries.txt", args = dependency_sources_jars),
            "--output",
            combined_sources_jar.as_output(),
        ])

        if ctx.attrs.remove_classes:
            remove_classes_file = ctx.actions.write("sources_remove_classes.txt", ctx.attrs.remove_classes, has_content_based_path = False)
            combined_sources_jar_cmd.add([
                "--blocklist-patterns",
                remove_classes_file,
                "--blocklist-patterns-matcher",
                "remove_classes_patterns_matcher",
            ])

        ctx.actions.run(combined_sources_jar_cmd, category = "create_sources_jar")
        sub_targets["sources.jar"] = [DefaultInfo(default_output = combined_sources_jar)]

    entries = [android_manifest, classes_jar]

    all_resource_infos = list(android_packageable_info.resource_infos.traverse()) if android_packageable_info.resource_infos else []
    resource_infos = [resource_info for resource_info in all_resource_infos if resource_info.raw_target not in excluded_android_packageable_targets]

    android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo]
    if resource_infos:
        res_dirs = [resource_info.res for resource_info in resource_infos if resource_info.res]
        if ctx.attrs.package_resources and res_dirs:
            merged_resource_sources_dir = ctx.actions.declare_output("merged_resource_sources_dir/res", dir = True, has_content_based_path = False)
            merge_resource_sources_cmd = cmd_args([
                android_toolchain.merge_android_resource_sources[RunInfo],
                "--resource-paths",
                argfile(actions = ctx.actions, name = "resource_paths.txt", args = res_dirs),
                "--output",
                merged_resource_sources_dir.as_output(),
            ])

            ctx.actions.run(merge_resource_sources_cmd, category = "merge_android_resource_sources")

            r_dot_txt = get_text_symbols(
                ctx, merged_resource_sources_dir, [dep for dep in deps if AndroidResourceInfo in dep or ExportedAndroidResourceInfo in dep]
            )
            entries.extend([merged_resource_sources_dir, r_dot_txt])

        assets_dirs = flatten([resource_infos.assets for resource_infos in resource_infos])
        entries.extend(assets_dirs)

    cxx_resources = get_cxx_resources(ctx, deps, dir_name = "assets")
    if cxx_resources:
        entries.append(cxx_resources)

    native_libs_file = argfile(actions = ctx.actions, name = "native_libs_entries.txt", args = android_binary_native_library_info.native_libs_for_primary_apk)
    native_libs_assets_file = argfile(
        actions = ctx.actions, name = "native_libs_assets_entries.txt", args = android_binary_native_library_info.root_module_native_lib_assets
    )

    entries_file = ctx.actions.write("entries.txt", entries, has_content_based_path = False)

    aar = ctx.actions.declare_output("{}.aar".format(ctx.label.name), has_content_based_path = False)
    create_aar_cmd = cmd_args(
        [
            android_toolchain.aar_builder,
            "--output_path",
            aar.as_output(),
            "--entries_file",
            entries_file,
            "--on_duplicate_entry",
            "fail",
            "--native_libs_file",
            native_libs_file,
            "--native_libs_assets_file",
            native_libs_assets_file,
        ],
        hidden = [
            entries,
        ],
    )
    if ctx.attrs.proguard_config:
        create_aar_cmd.add([
            "--proguard_config_file",
            ctx.attrs.proguard_config,
        ])

    if ctx.attrs.hardcode_permissions_for_deterministic_output == True:
        create_aar_cmd.add([
            "--hardcode_permissions_for_deterministic_output",
        ])

    ctx.actions.run(create_aar_cmd, category = "create_aar")

    target_stats_providers, target_stats_subtargets = target_stats_aggregate_providers_and_subtargets(ctx, deps = deps)
    sub_targets.update(target_stats_subtargets)

    return [DefaultInfo(default_outputs = [aar], sub_targets = enhancement_ctx.get_sub_targets() | sub_targets)] + target_stats_providers

android_aar = prelude_rule(
    name = "android_aar",
    impl = android_aar_impl,
    docs = """
        An `android_aar()` rule is used to generate an Android AAR.

        See the [official Android documentation](https://developer.android.com/studio/projects/android-library#aar-contents) for details about the `.aar` format.
    """,
    examples = """
        ```
        android_resource(
          name = 'res',
          res = 'res',
          assets = 'assets',
          package = 'com.example',
        )

        android_library(
          name = 'lib',
          srcs = glob(['**/*.java']),
        )

        android_aar(
          name = 'app',
          manifest_skeleton = 'AndroidManifestSkeleton.xml',
          deps = [
            ':res',
            ':lib',
          ],
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "manifest_skeleton": attrs.source(
                doc = """
                The skeleton manifest file used to generate the final `AndroidManifest.xml` . May either be a file or an `android_manifest()` target.
            """,
            ),
            "build_config_values": attrs.list(
                attrs.string(),
                default = [],
                doc = """
                See the documentation on the values argument for `android_build_config()`.
            """,
            ),
            "include_build_config_class": attrs.bool(
                default = False,
                doc = """
                Whether to include the `BuildConfig` class files in the final .aar file. Needs
                 to be set to `True` if any build\\_config\\_values are specified.
                 This is normally only needed if the build tool that is consuming the .aar file does not generate
                 `BuildConfig` classes.
                 Note: the AAR format does not specify a way to pass defaults that should be injected into
                 the final `BuildConfig` class, therefore that information might need to be replicated
                 manually in the build that's consuming the .aar file.
            """,
            ),
            "deps": attrs.list(
                attrs.split_transition_dep(cfg = cpu_split_transition),
                default = [],
                doc = """
                List of build targets whose corresponding compiled Java code,
                 Android resources, and native libraries will be included in the AAR along with their transitive
                 dependencies. For compile time deps which should not be included in the final AAR,
                 use `provided_deps` instead.
                 * `android_library()` Will be included in the final `classes.jar`* `android_resource()` Will be included in the final `R.txt`,
                 `res/` and `assets/`* `android_build_config()` Will be included in the final `classes.jar`
                 if `include_build_config_class` is True
                 * `groovy_library()` Will be included in the final `classes.jar`* `java_library()` Will be included in the final `classes.jar`* `prebuilt_jar()` Will be included in the final `classes.jar`* `prebuilt_native_library()` Will be included in the final `jni/` or
                 `assets/` if `is_asset` is True
            """,
            ),
            "remove_classes": attrs.list(
                attrs.regex(),
                default = [],
                doc = """
                List of classes to remove from the output aar. It removes classes from the target's own sources,
                 and its dependencies.
            """,
            ),
            "build_config_values_file": attrs.option(attrs.source(), default = None),
            "cpu_filters": attrs.list(attrs.enum(TargetCpuType), default = ALL_CPU_FILTERS),
            "enable_relinker": attrs.bool(default = False),
            "excluded_java_deps": attrs.list(attrs.dep(), default = []),
            "extra_arguments": attrs.list(attrs.string(), default = []),
            "extra_kotlinc_arguments": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
            "extra_relinker_outputs": attrs.list(attrs.string(), default = []),
            "preinline_ir_sonames": attrs.list(attrs.string(), default = []),
            # @oss-disable[end= ]: "gatorade_extra_args": attrs.list(attrs.arg(), default = [], doc = "Extra Gatorade cross-library step arguments"),
            # @oss-disable[end= ]: "gatorade_phases": attrs.set(attrs.enum(GatoradePhase), default = []),
            "hardcode_permissions_for_deterministic_output": attrs.option(
                attrs.bool(),
                default = None,
                doc = """
                If set to true, Buck hardcodes the permissions in order to ensures that all files have the same
                permissions regardless of the platform on which the zip was generated.
            """,
            ),
            "java_version": attrs.option(attrs.string(), default = None),
            "language": attrs.option(attrs.enum(JvmLanguage), default = None),
            "manifest": attrs.option(attrs.source(), default = None),
            "manifest_entries": attrs.dict(key = attrs.string(), value = attrs.any(), default = {}),
            "manifest_file": attrs.option(attrs.source(), default = None),
            "maven_coords": attrs.option(attrs.string(), default = None),
            "min_sdk_version": attrs.option(attrs.int(), default = None),
            "native_library_bolt_args": attrs.option(attrs.dict(key = attrs.string(), value = attrs.list(attrs.arg())), default = None),
            "native_library_merge_code_generator": attrs.option(attrs.exec_dep(), default = None),
            "native_library_merge_glue": attrs.option(attrs.split_transition_dep(cfg = cpu_split_transition), default = None),
            "native_library_merge_linker_args": attrs.option(attrs.dict(key = attrs.string(), value = attrs.list(attrs.arg())), default = None),
            "native_library_merge_linker_args_all": attrs.list(attrs.arg(), default = [], doc = "Extra linker arguments passed to all merged libraries."),
            "native_library_merge_map": attrs.option(attrs.dict(key = attrs.string(), value = attrs.list(attrs.regex()), sorted = False), default = None),
            "native_library_merge_non_asset_libs": attrs.bool(default = False),
            "native_library_merge_sequence": attrs.option(
                attrs.list(
                    attrs.one_of(
                        attrs.tuple(attrs.string(), attrs.list(attrs.regex())),
                        attrs.list(attrs.tuple(attrs.string(), attrs.list(attrs.regex()))),
                    ),
                ),
                default = None,
            ),
            "native_library_merge_sequence_blocklist": attrs.option(attrs.list(attrs.regex()), default = None),
            "package_asset_libraries": attrs.bool(default = True),
            "package_resources": attrs.bool(default = True),
            "proguard_config": attrs.option(attrs.source(), default = None),
            "relinker_extra_args": attrs.dict(
                key = attrs.string(),
                value = attrs.list(attrs.arg()),
                sorted = False,
                default = {},
                doc = "Per-library extra linker arguments passed when relinking, mapping sonames to lists of arguments.",
            ),
            "relinker_extra_args_all": attrs.list(attrs.arg(), default = [], doc = "Extra arguments passed when relinking all libraries."),
            "relinker_extra_deps": attrs.list(
                attrs.split_transition_dep(cfg = cpu_split_transition),
                default = [],
                doc = "Deps statically linked to every native lib by the relinker.",
            ),
            "relinker_whitelist": attrs.list(attrs.regex(), default = []),
            "required_for_source_only_abi": attrs.bool(default = False),
            "resources": attrs.list(attrs.source(), default = []),
            "resources_root": attrs.option(attrs.string(), default = None),
            "runtime_deps": attrs.list(attrs.dep(), default = []),
            "source": attrs.option(attrs.string(), default = None),
            "source_only_abi_deps": attrs.list(attrs.dep(), default = []),
            "srcs": attrs.list(attrs.source(), default = []),
            "strip_libraries": attrs.default_only(attrs.bool(default = not DISABLE_STRIPPING)),
            "target": attrs.option(attrs.string(), default = None),
            "_android_toolchain": toolchains_common.android(),
            "_cxx_toolchain": attrs.split_transition_dep(cfg = cpu_split_transition, default = "toolchains//:android-hack"),
            "_is_building_android_binary": attrs.default_only(attrs.bool(default = True)),
            "_java_toolchain": toolchains_common.java_for_android(),
        }
        | CPU_TRANSITION_ATTRS
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    )
    | jvm_common.abi_generation_mode()
    | jvm_common.annotation_processors()
    | jvm_common.plugins(),
)
