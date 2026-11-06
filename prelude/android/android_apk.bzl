# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//:validation_deps.bzl", "get_validation_deps_outputs")
load("@prelude//android:android_binary.bzl", "get_binary_info")
load(
    "@prelude//android:android_providers.bzl",
    "AndroidApkExopackageInfo",
    "AndroidApkInfo",
    "AndroidApkUnderTestInfo",
    "AndroidBinaryNativeLibsInfo",
    "AndroidBinaryPrimaryPlatformInfo",
    "AndroidBinaryResourcesInfo",
    "DexFilesInfo",
    "ExopackageInfo",
    "KeystoreInfo",
)
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:util.bzl", "package_validators_decorator")
load("@prelude//java/utils:java_more_utils.bzl", "get_path_separator_for_exec_os")
load("@prelude//target_stats:target_stats.bzl", "target_stats_aggregate_providers_and_subtargets")
load("@prelude//transitions:constraint_overrides.bzl", "constraint_overrides")
load("@prelude//utils:argfile.bzl", "argfile")
load("@prelude//utils:utils.bzl", "flatten")
load(":attrs.bzl", "ANDROID_BINARY_ATTRS")

def android_apk_impl(ctx: AnalysisContext) -> list[Provider]:
    android_binary_info = get_binary_info(ctx, use_proto_format = False)
    java_packaging_deps = android_binary_info.java_packaging_deps
    sub_targets = android_binary_info.sub_targets
    dex_files_info = android_binary_info.dex_files_info
    native_library_info = android_binary_info.native_library_info
    resources_info = android_binary_info.resources_info
    validation_outputs = android_binary_info.validation_outputs

    wrapped_build_apk = package_validators_decorator(
        ctx,
        build_apk,
        extension = ".apk",
    )

    keystore = ctx.attrs.keystore[KeystoreInfo]
    apk_kwargs = dict(
        actions = ctx.actions,
        android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo],
        keystore = keystore,
        dex_files_info = dex_files_info,
        resources_info = resources_info,
        compress_resources_dot_arsc = ctx.attrs.resource_compression == "enabled" or ctx.attrs.resource_compression == "enabled_with_strings_as_assets",
        packaging_options = ctx.attrs.packaging_options,
        # Exclude BUCK_BUILD_ID from assets/BuildInfo.json for release builds.
        include_build_info_file = ctx.attrs.include_build_info_file and ctx.attrs.package_type != "release",
    )

    # With a deferred relink, the apk this rule outputs is built from the relinked libraries, and
    # an early apk from the unrelinked ones lets Redex start before the relinks finish (the
    # [unrelinked_package] sub-target; the step after Redex swaps the relinked libraries in).
    unrelinked_apk = None
    if native_library_info.shipped:
        unrelinked_apk = build_apk(
            output_filename = ctx.label.name + "_unrelinked",
            native_library_info = native_library_info,
            validation_deps_outputs = get_validation_deps_outputs(ctx) + validation_outputs,
            name_prefix = "unrelinked/",
            **apk_kwargs,
        )
        validation_outputs = validation_outputs + native_library_info.shipped.validation_outputs
        native_library_info = native_library_info.shipped
    output_apk = wrapped_build_apk(
        output_filename = ctx.label.name,
        native_library_info = native_library_info,
        validation_deps_outputs = get_validation_deps_outputs(ctx) + validation_outputs,
        **apk_kwargs,
    )

    if dex_files_info.secondary_dex_exopackage_info or native_library_info.exopackage_info or resources_info.exopackage_info:
        exopackage_info = ExopackageInfo(
            secondary_dex_info = dex_files_info.secondary_dex_exopackage_info,
            native_library_info = native_library_info.exopackage_info,
            resources_info = resources_info.exopackage_info,
        )
        default_output = ctx.actions.write(
            "{}_exopackage_apk_warning".format(ctx.label.name),
            "exopackage apks should not be used externally, try buck install or building with exopackage disabled\n",
            has_content_based_path = False,
        )
        sub_targets["exo_apk"] = [DefaultInfo(default_output = output_apk)]  # Used by tests
    else:
        exopackage_info = None
        default_output = output_apk

    # We can only be sure that an APK has native libs if it has any shared libraries. Prebuilt native libraries dirs can exist but be empty.
    definitely_has_native_libs = bool(native_library_info.shared_libraries)

    install_info = get_install_info(
        ctx, output_apk = output_apk, manifest = resources_info.manifest, exopackage_info = exopackage_info, definitely_has_native_libs = definitely_has_native_libs
    )

    classpath = [dep.jar for dep in java_packaging_deps if dep.jar]
    sub_targets["classpath"] = [
        DefaultInfo(default_output = ctx.actions.write("classpath.txt", classpath, has_content_based_path = False), other_outputs = classpath)
    ]
    sub_targets["classpath_targets"] = [
        DefaultInfo(
            default_output = ctx.actions.write("classpath_targets.txt", list(set([jar.owner.raw_target() for jar in classpath])), has_content_based_path = False)
        )
    ]

    # ctx.attrs.deps is split-transitioned here (one Dependency per ABI).
    target_stats_providers, target_stats_subtargets = target_stats_aggregate_providers_and_subtargets(
        ctx,
        deps = android_binary_info.deps_by_platform[android_binary_info.primary_platform],
    )
    sub_targets.update(target_stats_subtargets)

    def apk_info(apk):
        return AndroidApkInfo(
            apk = apk,
            manifest = resources_info.manifest,
            materialized_artifacts = android_binary_info.materialized_artifacts,
            unstripped_shared_libraries = native_library_info.unstripped_shared_libraries,
        )

    apk_under_test_info = AndroidApkUnderTestInfo(
        java_packaging_deps = set([dep.label.raw_target() for dep in java_packaging_deps]),
        keystore = keystore,
        manifest_entries = ctx.attrs.manifest_entries,
        min_sdk_version = ctx.attrs.min_sdk_version,
        prebuilt_native_library_dirs = set([native_lib.raw_target for native_lib in native_library_info.prebuilt_native_library_dirs]),
        platform_configurations = set([str(x.label.configured_target().config()) for x in flatten(android_binary_info.deps_by_platform.values())]),
        platforms = android_binary_info.deps_by_platform.keys(),
        primary_platform = android_binary_info.primary_platform,
        resource_infos = set([info.raw_target for info in resources_info.unfiltered_resource_infos]),
        r_dot_java_packages = set([info.specified_r_dot_java_package for info in resources_info.unfiltered_resource_infos if info.specified_r_dot_java_package]),
        shared_libraries = set(native_library_info.shared_libraries),
        # Merge map delegate
        native_library_merge_sequence = ctx.attrs.native_library_merge_sequence,
        native_library_merge_code_generator = ctx.attrs.native_library_merge_code_generator,
        native_library_merge_glue = ctx.attrs.native_library_merge_glue,
        native_library_merge_linker_args_all = ctx.attrs.native_library_merge_linker_args_all,
        native_library_merge_linker_args = ctx.attrs.native_library_merge_linker_args,
        native_library_merge_map = ctx.attrs.native_library_merge_map,
        native_library_merge_non_asset_libs = ctx.attrs.native_library_merge_non_asset_libs,
        native_library_merge_sequence_blocklist = ctx.attrs.native_library_merge_sequence_blocklist,
    )
    template_placeholder_info = TemplatePlaceholderInfo(
        keyed_variables = {
            "classpath": cmd_args(classpath, delimiter = get_path_separator_for_exec_os(ctx)),
            "classpath_including_targets_with_no_output": cmd_args(
                [dep.output_for_classpath_macro for dep in java_packaging_deps], delimiter = get_path_separator_for_exec_os(ctx)
            ),
        },
    )
    preprocessed_java_classes_info = [android_binary_info.preprocessed_java_classes_info] if android_binary_info.preprocessed_java_classes_info else []

    # What an apk_genrule needs to run on the early package: the step that runs Redex on a
    # deferred relink's binary reads this, and the step after it swaps the relinked libraries in.
    # Without a deferred relink it is the apk this rule outputs.
    early_apk = unrelinked_apk or output_apk
    sub_targets["unrelinked_package"] = [
        apk_info(early_apk),
        apk_under_test_info,
        template_placeholder_info,
        android_binary_info.class_to_srcs,
        DefaultInfo(default_output = early_apk, sub_targets = dict(sub_targets)),
    ] + preprocessed_java_classes_info

    providers = (
        [
            apk_info(output_apk),
            AndroidBinaryPrimaryPlatformInfo(
                primary_platform = android_binary_info.primary_platform,
            ),
            apk_under_test_info,
            DefaultInfo(
                default_output = default_output,
                other_outputs = install_info.files.values() + android_binary_info.materialized_artifacts,
                sub_targets = sub_targets,
            ),
            install_info,
            template_placeholder_info,
            android_binary_info.class_to_srcs,
        ]
        + target_stats_providers
        + preprocessed_java_classes_info
    )

    # Expose the exopackage secondary-dex dir so android_instrumentation_test can push it to the device.
    if exopackage_info != None and exopackage_info.secondary_dex_info != None:
        providers.append(
            AndroidApkExopackageInfo(secondary_dex_directory = exopackage_info.secondary_dex_info.directory),
        )

    return providers

def build_apk(
    output_filename: str,
    actions: AnalysisActions,
    keystore: KeystoreInfo,
    android_toolchain: AndroidToolchainInfo,
    dex_files_info: DexFilesInfo,
    native_library_info: AndroidBinaryNativeLibsInfo,
    resources_info: AndroidBinaryResourcesInfo,
    compress_resources_dot_arsc: bool = False,
    validation_deps_outputs: [list[Artifact], None] = None,
    packaging_options: dict | None = None,
    include_build_info_file: bool = False,
    # Prefixes the files declared besides the apk, so one analysis can build two apks.
    name_prefix: str = "",
) -> Artifact:
    output_apk = actions.declare_output("{}.apk".format(output_filename), has_content_based_path = False)

    apk_builder_args = cmd_args(
        android_toolchain.apk_builder[RunInfo],
        "--output-apk",
        output_apk.as_output(),
        "--resource-apk",
        resources_info.primary_resources_apk,
        "--dex-file",
        dex_files_info.primary_dex,
        "--keystore-path",
        keystore.store,
        "--keystore-properties-path",
        keystore.properties,
        "--zipalign_tool",
        android_toolchain.zipalign[RunInfo],
        "--package-meta-inf-version-files" if android_toolchain.package_meta_inf_version_files else [],
        "--compress-resources-dot-arsc" if compress_resources_dot_arsc else [],
        # The outputs of validation_deps need to be added as hidden arguments
        # to an action for the validation_deps targets to be built and enforced.
        hidden = validation_deps_outputs or [],
    )

    asset_directories = (
        native_library_info.root_module_native_lib_assets
        + native_library_info.non_root_module_native_lib_assets
        + dex_files_info.root_module_bootstrap_dex_dirs
        + dex_files_info.root_module_secondary_dex_dirs
        + dex_files_info.non_root_module_secondary_dex_dirs
        + resources_info.module_manifests
    )
    asset_directories_file = argfile(actions = actions, name = name_prefix + "asset_directories.txt", args = asset_directories)
    native_library_directories = argfile(actions = actions, name = name_prefix + "native_library_directories", args = native_library_info.native_libs_for_primary_apk)
    all_zip_files = [resources_info.packaged_string_assets] if resources_info.packaged_string_assets else []
    zip_files = argfile(actions = actions, name = name_prefix + "zip_files", args = all_zip_files)
    jar_files_that_may_contain_resources = argfile(
        actions = actions, name = name_prefix + "jar_files_that_may_contain_resources", args = resources_info.jar_files_that_may_contain_resources
    )

    apk_builder_args.add([
        "--asset-directories-list",
        asset_directories_file,
        "--native-libraries-directories-list",
        native_library_directories,
        "--zip-files-list",
        zip_files,
        "--jar-files-that-may-contain-resources-list",
        jar_files_that_may_contain_resources,
    ])

    # Invoke the toolchain's build-info generator from this apk packaging action (which already has
    # every APK input) so the baked BUCK_BUILD_ID refreshes when the APK content changes and
    # cache-hits otherwise -- no separate action. Its run command is multi-token (java -jar ...), so
    # pass it via an argfile the apk-builder reads and execs.
    if include_build_info_file:
        apk_builder_args.add(
            "--build-info-generator-args",
            argfile(
                actions = actions,
                name = name_prefix + "build_info_generator_args",
                args = cmd_args(android_toolchain.build_info_generator[RunInfo]),
                allow_args = True,
            ),
        )

    if packaging_options:
        for key, value in packaging_options.items():
            if key == "excluded_resources":
                apk_builder_args.add("--excluded-resources", actions.write(name_prefix + "excluded_resources.txt", value, has_content_based_path = False))
            elif key == "uncompressed_files":
                apk_builder_args.add("--uncompressed-files", actions.write(name_prefix + "uncompressed_files.txt", value, has_content_based_path = False))
            else:
                fail("Only 'excluded_resources' and 'uncompressed_files' are supported in packaging_options right now!")

    actions.run(apk_builder_args, category = "apk_build", identifier = output_filename, allow_cache_upload = True)

    return output_apk

def get_install_info(
    ctx: AnalysisContext,
    output_apk: Artifact,
    manifest: Artifact,
    exopackage_info: [ExopackageInfo, None],
    definitely_has_native_libs: bool = True,
    apex_mode: bool = False,
) -> InstallInfo:
    files = {
        ctx.attrs.name: output_apk,
        "manifest": manifest,
        "options": generate_install_config(ctx, apex_mode),
    }

    if exopackage_info:
        secondary_dex_exopackage_info = exopackage_info.secondary_dex_info
        native_library_exopackage_info = exopackage_info.native_library_info
        resources_info = exopackage_info.resources_info
    else:
        secondary_dex_exopackage_info = None
        native_library_exopackage_info = None
        resources_info = None

    if secondary_dex_exopackage_info:
        files["secondary_dex_exopackage_info_directory"] = secondary_dex_exopackage_info.directory
        files["secondary_dex_exopackage_info_metadata"] = secondary_dex_exopackage_info.metadata

    if native_library_exopackage_info:
        files["native_library_exopackage_info_directory"] = native_library_exopackage_info.directory
        files["native_library_exopackage_info_metadata"] = native_library_exopackage_info.metadata

    if resources_info:
        if resources_info.assets:
            files["resources_exopackage_assets"] = resources_info.assets
            files["resources_exopackage_assets_hash"] = resources_info.assets_hash

        files["resources_exopackage_res"] = resources_info.res
        files["resources_exopackage_res_hash"] = resources_info.res_hash

    if definitely_has_native_libs and hasattr(ctx.attrs, "cpu_filters"):
        files["cpu_filters"] = ctx.actions.write("cpu_filters.txt", ctx.attrs.cpu_filters, has_content_based_path = False)

    return InstallInfo(
        installer = ctx.attrs._android_toolchain[AndroidToolchainInfo].installer,
        files = files,
    )

def generate_install_config(ctx: AnalysisContext, apex_mode: bool) -> Artifact:
    data = get_install_config(apex_mode)
    return ctx.actions.write_json("install_android_options.json", data, has_content_based_path = False)

def get_install_config(apex_mode: bool) -> dict[str, typing.Any]:
    # TODO: read from toolchains
    install_config = {
        "adb_restart_on_failure": read_root_config("adb", "adb_restart_on_failure", "true"),
        "apex_mode": apex_mode,
        "multi_install_mode": read_root_config("adb", "multi_install_mode", "false"),
        "staged_install_mode": read_root_config("adb", "staged_install_mode", None),
    }

    adb_executable = read_root_config("android", "adb", None)
    if adb_executable:
        install_config["adb_executable"] = adb_executable

    return install_config

android_binary = prelude_rule(
    name = "android_binary",
    impl = android_apk_impl,
    docs = """
        An `android_binary()` rule is used to generate an Android APK.
    """,
    examples = None,
    further = None,
    attrs = ANDROID_BINARY_ATTRS,
    cfg = constraint_overrides.transition,
)
