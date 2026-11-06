# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//:validation_deps.bzl", "get_validation_deps_outputs")
load("@prelude//android:android_apk.bzl", "get_install_info")
load("@prelude//android:android_binary.bzl", "get_binary_info")
load(
    "@prelude//android:android_providers.bzl",
    "AndroidAabInfo",
    "AndroidBinaryNativeLibsInfo",
    "AndroidBinaryPrimaryPlatformInfo",
    "AndroidBinaryResourcesInfo",
    "AndroidDerivedApkInfo",
    "DexFilesInfo",
    "KeystoreInfo",
)
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:bundletool_util.bzl", "derive_universal_apk")
load("@prelude//android:util.bzl", "package_validators_decorator")
load("@prelude//java/utils:java_more_utils.bzl", "get_path_separator_for_exec_os")
load("@prelude//target_stats:target_stats.bzl", "target_stats_aggregate_providers_and_subtargets")
load("@prelude//utils:argfile.bzl", "argfile")
load(":attrs.bzl", "ANDROID_BUNDLE_ATTRS")

def android_bundle_impl(ctx: AnalysisContext) -> list[Provider]:
    android_binary_info = get_binary_info(ctx, use_proto_format = True)
    native_library_info = android_binary_info.native_library_info

    wrapped_build_bundle = package_validators_decorator(
        ctx,
        build_bundle,
        extension = ".aab",
    )

    bundle_kwargs = dict(
        actions = ctx.actions,
        android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo],
        dex_files_info = android_binary_info.dex_files_info,
        resources_info = android_binary_info.resources_info,
        bundle_config = ctx.attrs.bundle_config_file,
        packaging_options = ctx.attrs.packaging_options,
    )

    # With a deferred relink, the bundle this rule outputs is built from the relinked libraries,
    # and an early bundle from the unrelinked ones lets Redex start before the relinks finish (the
    # [unrelinked_package] sub-target; the step after Redex swaps the relinked libraries in).
    unrelinked_bundle = None
    if native_library_info.shipped:
        unrelinked_bundle = build_bundle(
            output_filename = ctx.label.name + "_unrelinked",
            native_library_info = native_library_info,
            validation_deps_outputs = get_validation_deps_outputs(ctx) + native_library_info.validation_outputs,
            name_prefix = "unrelinked/",
            **bundle_kwargs,
        )
        native_library_info = native_library_info.shipped
    output_bundle = wrapped_build_bundle(
        output_filename = ctx.label.name,
        native_library_info = native_library_info,
        validation_deps_outputs = get_validation_deps_outputs(ctx) + native_library_info.validation_outputs,
        **bundle_kwargs,
    )

    extra_providers = []
    if android_binary_info.preprocessed_java_classes_info:
        extra_providers.append(android_binary_info.preprocessed_java_classes_info)
    sub_targets = {}
    sub_targets.update(android_binary_info.sub_targets)
    if ctx.attrs.use_derived_apk:
        keystore = ctx.attrs.keystore[KeystoreInfo]
        default_output = derive_universal_apk(
            ctx,
            android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo],
            app_bundle = output_bundle,
            keystore = keystore,
        )
        extra_providers.append(AndroidDerivedApkInfo(apk = default_output))
        extra_providers.append(
            get_install_info(
                ctx,
                exopackage_info = None,
                manifest = android_binary_info.resources_info.manifest,
                output_apk = default_output,
            ),
        )
        sub_targets["aab"] = [
            DefaultInfo(
                default_outputs = [output_bundle],
            )
        ]
    else:
        default_output = output_bundle

    target_stats_providers, target_stats_subtargets = target_stats_aggregate_providers_and_subtargets(
        ctx,
        deps = android_binary_info.deps_by_platform[android_binary_info.primary_platform],
    )
    sub_targets.update(target_stats_subtargets)

    java_packaging_deps = android_binary_info.java_packaging_deps

    def aab_info(aab):
        return AndroidAabInfo(
            aab = aab,
            manifest = android_binary_info.resources_info.manifest,
            materialized_artifacts = android_binary_info.materialized_artifacts,
            unstripped_shared_libraries = native_library_info.unstripped_shared_libraries,
        )

    # What an apk_genrule needs to run on the early bundle (see android_apk_impl).
    early_bundle = unrelinked_bundle or output_bundle
    early_sub_targets = dict(sub_targets)
    if "aab" in early_sub_targets:
        early_sub_targets["aab"] = [DefaultInfo(default_outputs = [early_bundle])]
    sub_targets["unrelinked_package"] = [
        aab_info(early_bundle),
        DefaultInfo(default_output = early_bundle, sub_targets = early_sub_targets),
    ] + ([android_binary_info.preprocessed_java_classes_info] if android_binary_info.preprocessed_java_classes_info else [])

    return (
        [
            DefaultInfo(default_output = default_output, other_outputs = android_binary_info.materialized_artifacts, sub_targets = sub_targets),
            aab_info(output_bundle),
            AndroidBinaryPrimaryPlatformInfo(
                primary_platform = android_binary_info.primary_platform,
            ),
            TemplatePlaceholderInfo(
                keyed_variables = {
                    "classpath": cmd_args([dep.jar for dep in java_packaging_deps if dep.jar], delimiter = get_path_separator_for_exec_os(ctx)),
                    "classpath_including_targets_with_no_output": cmd_args(
                        [dep.output_for_classpath_macro for dep in java_packaging_deps], delimiter = get_path_separator_for_exec_os(ctx)
                    ),
                },
            ),
        ]
        + extra_providers
        + target_stats_providers
    )

def build_bundle(
    output_filename: str,
    actions: AnalysisActions,
    android_toolchain: AndroidToolchainInfo,
    dex_files_info: DexFilesInfo,
    native_library_info: AndroidBinaryNativeLibsInfo,
    resources_info: AndroidBinaryResourcesInfo,
    bundle_config: Artifact | None,
    validation_deps_outputs: [list[Artifact], None] = None,
    packaging_options: dict | None = None,
    # Prefixes the files declared besides the bundle, so one analysis can build two bundles.
    name_prefix: str = "",
) -> Artifact:
    output_bundle = actions.declare_output("{}.aab".format(output_filename), has_content_based_path = False)

    bundle_builder_args = cmd_args(
        android_toolchain.bundle_builder[RunInfo],
        "--output-bundle",
        output_bundle.as_output(),
        "--resource-apk",
        resources_info.primary_resources_apk,
        "--dex-file",
        dex_files_info.primary_dex,
        # The outputs of validation_deps need to be added as hidden arguments
        # to an action for the validation_deps targets to be built and enforced.
        hidden = validation_deps_outputs or [],
    )

    if bundle_config:
        bundle_builder_args.add(["--path-to-bundle-config-file", bundle_config])

    if android_toolchain.package_meta_inf_version_files:
        bundle_builder_args.add("--package-meta-inf-version-files")

    root_module_asset_directories = (
        native_library_info.root_module_native_lib_assets + dex_files_info.root_module_bootstrap_dex_dirs + dex_files_info.root_module_secondary_dex_dirs
    )
    root_module_asset_directories_file = argfile(actions = actions, name = name_prefix + "root_module_asset_directories.txt", args = root_module_asset_directories)

    non_root_module_asset_directories = resources_info.module_manifests + dex_files_info.non_root_module_secondary_dex_dirs
    non_root_module_asset_directories_file = argfile(
        actions = actions, name = name_prefix + "non_root_module_asset_directories.txt", args = non_root_module_asset_directories
    )
    non_root_module_asset_native_lib_directories = argfile(
        actions = actions, name = name_prefix + "non_root_module_asset_native_lib_directories.txt", args = native_library_info.non_root_module_native_lib_assets
    )

    native_library_directories = argfile(actions = actions, name = name_prefix + "native_library_directories", args = native_library_info.native_libs_for_primary_apk)
    all_zip_files = [resources_info.packaged_string_assets] if resources_info.packaged_string_assets else []
    zip_files = argfile(actions = actions, name = name_prefix + "zip_files", args = all_zip_files)
    jar_files_that_may_contain_resources = argfile(
        actions = actions, name = name_prefix + "jar_files_that_may_contain_resources", args = resources_info.jar_files_that_may_contain_resources
    )

    if resources_info.module_assets:
        bundle_builder_args.add(["--module-assets-dir", resources_info.module_assets])

    bundle_builder_args.add([
        "--root-module-asset-directories-list",
        root_module_asset_directories_file,
        "--non-root-module-asset-directories-list",
        non_root_module_asset_directories_file,
        "--non-root-module-asset-native-lib-directories-list",
        non_root_module_asset_native_lib_directories,
        "--native-libraries-directories-list",
        native_library_directories,
        "--zip-files-list",
        zip_files,
        "--jar-files-that-may-contain-resources-list",
        jar_files_that_may_contain_resources,
    ])

    if packaging_options:
        for key, value in packaging_options.items():
            if key != "excluded_resources":
                fail("Only 'excluded_resources' is supported in packaging_options right now!")
            else:
                bundle_builder_args.add("--excluded-resources", actions.write(name_prefix + "excluded_resources.txt", value, has_content_based_path = False))

    actions.run(bundle_builder_args, category = "bundle_build", identifier = output_filename)

    return output_bundle

android_bundle = prelude_rule(
    name = "android_bundle",
    impl = android_bundle_impl,
    docs = """
        An `android_bundle()` rule is used to generate an Android App Bundle
        (AAB). It shares most attributes with `android_binary()`, but produces
        an AAB file instead of an APK. App Bundles are the recommended
        publishing format for Google Play, enabling optimized APK delivery
        per device configuration.
    """,
    examples = None,
    further = None,
    attrs = ANDROID_BUNDLE_ATTRS,
)
