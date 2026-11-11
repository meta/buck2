# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//android:android_apk.bzl", "build_apk")
load("@prelude//android:android_binary_native_library_rules.bzl", "get_android_binary_native_library_info")
load("@prelude//android:android_binary_resources_rules.bzl", "get_android_binary_resources_info")
load("@prelude//android:android_providers.bzl", "AndroidApkInfo", "AndroidApkUnderTestInfo", "AndroidInstrumentationApkInfo", "merge_android_packageable_info")
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:configuration.bzl", "CPU_TRANSITION_ATTRS", "cpu_split_transition", "cpu_transition", "get_deps_by_platform")
load("@prelude//android:dex_rules.bzl", "get_multi_dex", "get_single_primary_dex", "get_split_dex_merge_config", "merge_to_single_dex", "merge_to_split_dex")
load("@prelude//android:preprocess_java_classes.bzl", "get_preprocessed_java_classes")
load("@prelude//android:util.bzl", "create_enhancement_context")
load("@prelude//decls:common.bzl", "TargetCpuType", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//java:class_to_srcs.bzl", "merge_class_to_source_map_from_jar")
load("@prelude//java:java_providers.bzl", "create_java_packaging_dep", "get_all_java_packaging_deps")
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")
load("@prelude//java/utils:java_utils.bzl", "get_class_to_source_map_info")
load("@prelude//target_stats:target_stats.bzl", "target_stats_aggregate_providers_and_subtargets")
load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:utils.bzl", "flatten")
load(":attrs.bzl", "android_common")

def android_instrumentation_apk_impl(ctx: AnalysisContext):
    # jar preprocessing cannot be used when the jars were dexed already, so we have to disable predex when we want to preprocess the jars.
    disable_pre_dex = ctx.attrs.disable_pre_dex or ctx.attrs.preprocess_java_classes_bash

    apk_under_test_info = ctx.attrs.apk.get(AndroidApkUnderTestInfo)
    expect(apk_under_test_info != None, "APK-under-test must provide AndroidApkUnderTestInfo")

    expect(
        ctx.attrs.min_sdk_version == apk_under_test_info.min_sdk_version,
        """
Android instrumentation APK must have the same min_sdk_version as the APK-under-test!
This is important because the min_sdk_version affects the configuration of all deps.
Instrumentation APK min_sdk_version: {}, APK-under-test min_sdk_version: {}""".format(
            ctx.attrs.min_sdk_version,
            apk_under_test_info.min_sdk_version,
        ),
    )

    # android_instrumentation_apk uses the same platforms as the APK-under-test
    unfiltered_deps_by_platform = get_deps_by_platform(ctx)
    for platform in apk_under_test_info.platforms:
        expect(
            platform in unfiltered_deps_by_platform,
            "Android instrumentation APK must have any platforms that are in the APK-under-test!",
        )

    test_apk_platform_configurations = set([str(x.label.configured_target().config()) for x in flatten(unfiltered_deps_by_platform.values())])
    for platform_configuration in apk_under_test_info.platform_configurations:
        expect(
            platform_configuration in test_apk_platform_configurations,
            """
APK-under-test has deps that are configured in a different manner than the Android instrumentation APK!
This will lead to overbuilding and is not supported. Configuration {} not found in {}""".format(platform_configuration, test_apk_platform_configurations),
        )
    filtered_deps_by_platform = {platform: deps for platform, deps in unfiltered_deps_by_platform.items() if platform in apk_under_test_info.platforms}

    # We use the deps that don't have _build_only_native_code = True
    deps = unfiltered_deps_by_platform.values()[0]

    is_self_instrumenting = ctx.attrs.is_self_instrumenting

    all_java_packaging_deps = [packaging_dep for packaging_dep in get_all_java_packaging_deps(ctx, deps) if packaging_dep.dex]
    java_packaging_deps = [
        packaging_dep
        for packaging_dep in all_java_packaging_deps
        if is_self_instrumenting or packaging_dep.label.raw_target() not in apk_under_test_info.java_packaging_deps
    ]
    desugar_classpath_jars = [
        packaging_dep.jar
        for packaging_dep in all_java_packaging_deps
        if not is_self_instrumenting and packaging_dep.label.raw_target() in apk_under_test_info.java_packaging_deps
    ]

    android_packageable_info = merge_android_packageable_info(ctx.label, ctx.actions, deps)

    resources_info = get_android_binary_resources_info(
        ctx,
        deps,
        android_packageable_info,
        java_packaging_deps = java_packaging_deps,
        use_proto_format = False,
        referenced_resources_lists = [],
        manifest_entries = apk_under_test_info.manifest_entries,
        resource_infos_to_exclude = apk_under_test_info.resource_infos if not is_self_instrumenting else set(),
        r_dot_java_packages_to_exclude = apk_under_test_info.r_dot_java_packages if not is_self_instrumenting else set(),
    )
    android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo]
    java_packaging_deps += [
        create_java_packaging_dep(
            ctx,
            r_dot_java.library_info.library_output.full_library,
            dex_weight_factor = android_toolchain.r_dot_java_weight_factor,
        )
        for r_dot_java in resources_info.r_dot_java_infos
    ]

    enhance_ctx = create_enhancement_context(ctx)
    sub_targets = enhance_ctx.get_sub_targets()
    materialized_artifacts = []
    preprocessed_java_classes_info = None
    if not disable_pre_dex:
        pre_dexed_libs = [java_packaging_dep.dex for java_packaging_dep in java_packaging_deps]
        if ctx.attrs.use_split_dex:
            dex_merge_config = get_split_dex_merge_config(ctx, android_toolchain)
            dex_files_info = merge_to_split_dex(
                ctx,
                android_toolchain,
                pre_dexed_libs,
                dex_merge_config,
                enable_bootstrap_dexes = ctx.attrs.enable_bootstrap_dexes,
            )
        else:
            dex_files_info = merge_to_single_dex(ctx, android_toolchain, pre_dexed_libs)
    else:
        jars_to_owners = {packaging_dep.jar: packaging_dep.jar.owner.raw_target() for packaging_dep in java_packaging_deps}
        if ctx.attrs.preprocess_java_classes_bash:
            jars_to_owners, preprocessed_java_classes_info = get_preprocessed_java_classes(enhance_ctx, jars_to_owners)
            if preprocessed_java_classes_info:
                materialized_artifacts.append(preprocessed_java_classes_info.materialized_artifacts_dir)
        if ctx.attrs.use_split_dex:
            dex_files_info = get_multi_dex(
                ctx,
                ctx.attrs._android_toolchain[AndroidToolchainInfo],
                jars_to_owners,
                ctx.attrs.primary_dex_patterns,
                enable_bootstrap_dexes = ctx.attrs.enable_bootstrap_dexes,
                multidex_min_api = ctx.attrs.multidex_min_api,
                classpath_jars = desugar_classpath_jars,
            )
        else:
            dex_files_info = get_single_primary_dex(
                ctx,
                ctx.attrs._android_toolchain[AndroidToolchainInfo],
                jars_to_owners.keys(),
                classpath_jars = desugar_classpath_jars,
            )
    native_library_info = get_android_binary_native_library_info(
        enhance_ctx,
        android_packageable_info,
        filtered_deps_by_platform,
        prebuilt_native_library_dirs_to_exclude = apk_under_test_info.prebuilt_native_library_dirs if not is_self_instrumenting else set(),
        shared_libraries_to_exclude = apk_under_test_info.shared_libraries if not is_self_instrumenting else set(),
        native_library_merge_glue = apk_under_test_info.native_library_merge_glue,
        native_library_merge_code_generator = apk_under_test_info.native_library_merge_code_generator,
        native_library_merge_linker_args_all = apk_under_test_info.native_library_merge_linker_args_all,
        native_library_merge_linker_args = apk_under_test_info.native_library_merge_linker_args,
        native_library_merge_map = apk_under_test_info.native_library_merge_map,
        native_library_merge_non_asset_libs = apk_under_test_info.native_library_merge_non_asset_libs,
        native_library_merge_sequence = apk_under_test_info.native_library_merge_sequence,
        native_library_merge_sequence_blocklist = apk_under_test_info.native_library_merge_sequence_blocklist,
    )

    output_apk = build_apk(
        output_filename = ctx.label.name,
        actions = ctx.actions,
        android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo],
        keystore = apk_under_test_info.keystore,
        dex_files_info = dex_files_info,
        native_library_info = native_library_info,
        resources_info = resources_info,
    )

    class_to_srcs, _, class_to_srcs_subtargets = get_class_to_source_map_info(
        ctx,
        outputs = None,
        deps = deps,
    )
    transitive_class_to_src_map = merge_class_to_source_map_from_jar(
        actions = ctx.actions,
        name = ctx.label.name + ".transitive_class_to_src.json",
        java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo],
        relative_to = None,
        deps = [class_to_srcs],
    )
    sub_targets["transitive_class_to_src_map"] = [DefaultInfo(default_output = transitive_class_to_src_map)]

    target_stats_providers, target_stats_subtargets = target_stats_aggregate_providers_and_subtargets(ctx, deps = deps)
    sub_targets.update(target_stats_subtargets)

    return (
        [
            AndroidApkInfo(apk = output_apk, materialized_artifacts = materialized_artifacts, manifest = resources_info.manifest),
            AndroidInstrumentationApkInfo(
                apk_under_test = ctx.attrs.apk[AndroidApkInfo].apk,
                is_self_instrumenting = is_self_instrumenting,
                resource_source_infos = resources_info.unfiltered_resource_infos if is_self_instrumenting else apk_under_test_info.resource_source_infos,
            ),
            DefaultInfo(default_output = output_apk, other_outputs = materialized_artifacts, sub_targets = sub_targets | class_to_srcs_subtargets),
            class_to_srcs,
        ]
        + ([preprocessed_java_classes_info] if preprocessed_java_classes_info else [])
        + target_stats_providers
    )

android_instrumentation_apk = prelude_rule(
    name = "android_instrumentation_apk",
    impl = android_instrumentation_apk_impl,
    docs = """
        An `android_instrumentation_apk()` rule is used to generate
        an Android Instrumentation APK.

        Android's [Testing Fundamentals](http://developer.android.com/tools/testing/testing_android.html) documentation includes a diagram that shows
        the relationship between an "application package" and a "test package"
        when running a test. This rule corresponds to a test package. Note
        that a test package has an interesting quirk where it is *compiled
        against* an application package, but *must not include* the
        resources or Java classes of the application package. Therefore, this
        class takes responsibility for making sure the appropriate bits are
        excluded. Failing to do so will generate mysterious runtime errors
        when running the test.
    """,
    examples = """
        Here is an example of an `android_instrumentation_apk()` rule that tests an `android_binary()`, and depends on a test
        package.

        ```
        android_library(
          name = 'test',
          srcs = glob(['test/**/*.java']),
        )

        android_binary(
          name = 'messenger',
          manifest = 'AndroidManifest.xml',
          keystore = '//keystores:prod',
          package_type = 'release',
          proguard_config = 'proguard.cfg',
          deps = [
            ...
          ],
        )

        # Building this rule will produce a file named messenger_test.apk
        android_instrumentation_apk(
          name = 'messenger_test',
          manifest = 'AndroidInstrumentationManifest.xml',
          apk = ':messenger',
          deps = [
            ':test',
          ],
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        android_common.manifest_apk_arg()
        | {
            "apk": attrs.dep(
                doc = """
                APK build target, which should be used for the instrumentation APK.
                 Can be either an `android_binary()` or an `apk_genrule()`.
            """,
            ),
        }
        | android_common.deps_apk_arg()
        | {
            "cpu_filters": attrs.list(attrs.enum(TargetCpuType), default = []),
            "deps": attrs.list(
                attrs.split_transition_dep(cfg = cpu_split_transition),
                default = [],
                doc = """
                List of build targets whose corresponding compiled Java code,
                 Android resources, and native libraries will be included in the APK.
                 From the transitive closure of these dependencies, the outputs of rules of the following type will be included in the APK:
                 * `android_library()`* `android_resource()`* `cxx_library()`* `groovy_library()`* `java_library()`* `java_binary()`* `prebuilt_jar()`* `ndk_library()`* `prebuilt_native_library()`
            """,
            ),
            "disable_pre_dex": attrs.bool(default = False),
            "enable_bootstrap_dexes": attrs.bool(default = False),
            "includes_vector_drawables": attrs.bool(default = False),
            "is_self_instrumenting": attrs.bool(default = False),
            "manifest": attrs.option(
                attrs.one_of(attrs.transition_dep(cfg = cpu_transition), attrs.source()),
                default = None,
                doc = """
                Relative path to the Android manifest for the APK. The common
                 case is that the manifest will be in the same directory as the
                 rule, in which case this will simply be
                 `'AndroidManifest.xml'`, but it can also reference
                 an `android_manifest()` rule.

                 Prefer using `manifest_skeleton`, which performs merging automatically.
                 Exactly one of `manifest` and `manifest_skeleton` must be set.
            """,
            ),
            "manifest_skeleton": attrs.option(
                attrs.one_of(attrs.transition_dep(cfg = cpu_transition), attrs.source()),
                default = None,
                doc = """
                Relative path to the skeleton Android manifest for the APK.
                 An `android_manifest()` will be created automatically to merge
                 all manifests from libraries and resources going into the app.
                 The common case is that the manifest will be in the same directory as the
                 rule, in which case this will simply be
                 `'AndroidManifest.xml'`.

                 Exactly one of `manifest` and `manifest_skeleton` must be set.
            """,
            ),
            "min_sdk_version": attrs.option(attrs.int(), default = None),
            "multidex_min_api": attrs.option(attrs.string(), default = None),
            "preprocess_java_classes_bash": attrs.option(attrs.arg(), default = None),
            "preprocess_java_classes_deps": attrs.list(attrs.dep(), default = []),
            "primary_dex_patterns": attrs.list(attrs.string(), default = []),
            "use_split_dex": attrs.option(attrs.bool(), default = None),
            "_android_toolchain": toolchains_common.android(),
            "_cxx_toolchain": attrs.split_transition_dep(cfg = cpu_split_transition, default = "toolchains//:android-hack"),
            "_dex_toolchain": toolchains_common.dex(),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_is_building_android_binary": attrs.default_only(attrs.bool(default = True)),
            "_java_toolchain": toolchains_common.java_for_android(),
        }
        | CPU_TRANSITION_ATTRS
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
