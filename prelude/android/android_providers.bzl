# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//java:java_providers.bzl", "JavaLibraryInfo")

Aapt2LinkInfo = record(
    # "APK" containing resources to be used by the Android binary
    primary_resources_apk = Artifact,
    # proguard config needed to retain used resources
    proguard_config_file = Artifact,
    # R.txt containing all the linked resources
    r_dot_txt = Artifact,
)

PrebuiltNativeLibraryDir = record(
    raw_target = TargetLabel,
    dir = Artifact,  # contains subdirectories for different ABIs.
    for_primary_apk = bool,
    is_asset = bool,
)

ExopackageDexInfo = record(
    metadata = Artifact,
    directory = Artifact,
)

ExopackageNativeInfo = record(
    metadata = Artifact,
    directory = Artifact,
)

ExopackageResourcesInfo = record(
    assets = Artifact | None,
    assets_hash = Artifact | None,
    res = Artifact,
    res_hash = Artifact,
)

RDotJavaInfo = record(
    identifier = str,
    library_info = JavaLibraryInfo,
    source_zipped = Artifact,
)

AndroidBinaryNativeLibsInfo = record(
    prebuilt_native_library_dirs = list[PrebuiltNativeLibraryDir],
    # Indicates which shared lib producing targets are included in the binary. Used by instrumentation tests
    # to exclude those from the test apk.
    shared_libraries = list[TargetLabel],
    exopackage_info = [ExopackageNativeInfo, None],
    root_module_native_lib_assets = list[Artifact],
    non_root_module_native_lib_assets = list[Artifact],
    native_libs_for_primary_apk = list[Artifact],
    generated_java_code = list[Artifact],
    unstripped_shared_libraries = [Artifact, None],
    # Artifacts that must be produced for the binary to be valid. Threaded into
    # the apk's validation_deps_outputs, so a failing check fails the build.
    validation_outputs = list[Artifact],
    # Under defer_relink, the same packaging inputs built from the relinked
    # libraries (an AndroidBinaryNativeLibsInfo): the binary's own package is
    # built from them, while the inputs above, from the unrelinked libraries,
    # make the early package Redex starts from. None otherwise.
    shipped = field(typing.Any, None),
)

AndroidBinaryResourcesInfo = record(
    # Optional information about resources that should be exopackaged
    exopackage_info = [ExopackageResourcesInfo, None],
    # manifest to be used by the APK
    manifest = Artifact,
    # per-module manifests (packaged as assets)
    module_manifests = list[Artifact],
    # per-module assets APKs (for .aabs only)
    module_assets = Artifact | None,
    # zip containing any strings packaged as assets
    packaged_string_assets = Artifact | None,
    # "APK" containing resources to be used by the Android binary
    primary_resources_apk = Artifact,
    # proguard config needed to retain used resources
    proguard_config_file = Artifact,
    # R.java jars containing all the linked resources
    r_dot_java_infos = list[RDotJavaInfo],
    # directory containing filtered string resources files
    string_source_map = Artifact | None,
    # directory containing filtered string resources files for Voltron language packs
    voltron_string_source_map = Artifact | None,
    # list of jars that could contain resources that should be packaged into the APK
    jar_files_that_may_contain_resources = list[Artifact],
    # The resource infos that are used in this APK
    unfiltered_resource_infos = list,  # list[AndroidResourceInfo]
)

# Information about an `android_build_config`
BuildConfigField = record(
    type = str,
    name = str,
    value = str,
)

AndroidBuildConfigInfo = provider(
    # @unsorted-dict-items
    fields = {
        "package": str,
        "build_config_fields": list[BuildConfigField],
        "values_file": provider_field(Artifact | None, default = None),
    },
)

# Information about an `android_manifest`
AndroidManifestInfo = provider(
    fields = {
        "manifest": provider_field(Artifact),
        "merge_report": provider_field(Artifact),
    },
)

AndroidApkInfo = provider(
    fields = {
        "apk": provider_field(Artifact),
        "manifest": provider_field(Artifact),
        "materialized_artifacts": provider_field(list[Artifact]),
        "unstripped_shared_libraries": provider_field(Artifact | None, default = None),
    },
)

AndroidPreprocessedJavaClassesInfo = provider(
    fields = {
        "input_dir": provider_field(Artifact),
        "materialized_artifacts_dir": provider_field(Artifact),
    },
)

# Exopackage secondary-dex dir (metadata.txt + secondary-N.dex.jar) so android_instrumentation_test
# can push it to the device. Only set when the apk is built with exopackage secondary_dex mode.
AndroidApkExopackageInfo = provider(
    fields = {
        "secondary_dex_directory": provider_field(Artifact),
    },
)

AndroidAabInfo = provider(
    fields = {
        "aab": provider_field(Artifact),
        "manifest": provider_field(Artifact),
        "materialized_artifacts": provider_field(list[Artifact]),
        "unstripped_shared_libraries": provider_field(Artifact | None, default = None),
    },
)

AndroidDerivedApkInfo = provider(
    fields = {
        "apk": provider_field(Artifact),
    },
)

AndroidBinaryPrimaryPlatformInfo = provider(
    fields = {
        "primary_platform": provider_field(typing.Any, default = None),  # str
    },
)

AndroidApkUnderTestInfo = provider(
    # @unsorted-dict-items
    fields = {
        "java_packaging_deps": provider_field(typing.Any, default = None),  # set(TargetLabel)
        "keystore": provider_field(typing.Any, default = None),  # "KeystoreInfo"
        "manifest_entries": provider_field(typing.Any, default = None),  # dict
        "min_sdk_version": provider_field(typing.Any, default = None),  # int
        "platform_configurations": provider_field(typing.Any, default = None),  # [str]
        "platforms": provider_field(typing.Any, default = None),  # [str]
        "prebuilt_native_library_dirs": provider_field(typing.Any, default = None),  # set(TargetLabel)
        "primary_platform": provider_field(typing.Any, default = None),  # str
        "resource_infos": provider_field(typing.Any, default = None),  # set(TargetLabel)
        "r_dot_java_packages": provider_field(typing.Any, default = None),  # set(str)
        "shared_libraries": provider_field(typing.Any, default = None),  # set(TargetLabel)
        # Merge map delegate
        "native_library_merge_code_generator": provider_field(typing.Any, default = None),
        "native_library_merge_glue": provider_field(typing.Any, default = None),
        "native_library_merge_linker_args_all": provider_field(typing.Any, default = None),
        "native_library_merge_map": provider_field(typing.Any, default = None),
        "native_library_merge_non_asset_libs": provider_field(typing.Any, default = False),
        "native_library_merge_linker_args": provider_field(typing.Any, default = None),
        "native_library_merge_sequence": provider_field(typing.Any, default = None),
        "native_library_merge_sequence_blocklist": provider_field(typing.Any, default = None),
    },
)

AndroidInstrumentationApkInfo = provider(
    fields = {
        "apk_under_test": provider_field(typing.Any, default = None),  # "artifact"
        "is_self_instrumenting": provider_field(bool, default = False),
    },
)

KeystoreInfo = provider(
    # @unsorted-dict-items
    fields = {
        "store": provider_field(Artifact),
        "properties": provider_field(Artifact),
    },
)

ManifestInfo = record(
    target_label = TargetLabel,
    manifest = Artifact,
)

def _artifacts(value: ManifestInfo):
    return value.manifest

AndroidBuildConfigInfoTSet = transitive_set()
AndroidDepsTSet = transitive_set()
ManifestTSet = transitive_set(args_projections = {"artifacts": _artifacts})
PrebuiltNativeLibraryDirTSet = transitive_set()
ResourceInfoTSet = transitive_set()

DepsInfo = record(
    name = TargetLabel,
    deps = list[TargetLabel],
    for_primary_apk = bool,
)

AndroidPackageableInfo = provider(
    # @unsorted-dict-items
    fields = {
        "target_label": provider_field(typing.Any, default = None),  # "target_label"
        "build_config_infos": provider_field(typing.Any, default = None),  # ["AndroidBuildConfigInfoTSet", None]
        "deps": provider_field(typing.Any, default = None),  # ["AndroidDepsTSet", None]
        "manifests": provider_field(typing.Any, default = None),  # ["ManifestTSet", None]
        "prebuilt_native_library_dirs": provider_field(typing.Any, default = None),  # ["PrebuiltNativeLibraryDirTSet", None]
        "resource_infos": provider_field(typing.Any, default = None),  # ["AndroidResourceInfoTSet", None]
    },
)

RESOURCE_PRIORITY_NORMAL = "normal"
RESOURCE_PRIORITY_LOW = "low"

# Information about an `android_resource`
AndroidResourceInfo = provider(
    # @unsorted-dict-items
    fields = {
        # Target that produced this provider
        "raw_target": provider_field(typing.Any, default = None),  # TargetLabel
        # output of running `aapt2_compile` on the resources, if resources are present
        "aapt2_compile_output": provider_field(Artifact | None, default = None),
        # locales that should always be included in the APK for this resource
        "allowlisted_locales": provider_field(typing.Any, default = []),  # List
        #  if False, then the "res" are not affected by the strings-as-assets resource filter
        "allow_strings_as_assets_resource_filtering": provider_field(typing.Any, default = None),  # bool
        # assets defined by this rule. May be empty
        "assets": provider_field(list[Artifact], default = []),
        # manifest file used by the resources, if resources are present
        "manifest_file": provider_field(Artifact | None, default = None),
        # the package specified by the android_resource rule itself
        "specified_r_dot_java_package": provider_field(typing.Any, default = None),  # str | None
        # package used for R.java, if resources are present
        "r_dot_java_package": provider_field(Artifact | None, default = None),
        # resources defined by this rule. May be empty
        "res": provider_field(Artifact | None, default = None),
        # original resources before transforms, when available
        "source_res": provider_field(Artifact | None, default = None),
        # whether this resource contains non-XML resources
        "unused_resource_dep_validation_has_non_xml_resources": provider_field(bool, default = False),
        # priority of the resources, may be 'low' or 'normal'
        "res_priority": provider_field(typing.Any, default = None),  # str
        # symbols defined by the resources, if resources are present
        "text_symbols": provider_field(Artifact | None, default = None),
    },
)

# `AndroidResourceInfos` that are exposed via `exported_deps`
ExportedAndroidResourceInfo = provider(
    fields = {
        "resource_infos": provider_field(typing.Any, default = None),  # ["AndroidResourceInfo"]
    },
)

DexFilesInfo = record(
    primary_dex = Artifact,
    primary_dex_class_names = Artifact | None,
    root_module_bootstrap_dex_dirs = list[Artifact],
    root_module_secondary_dex_dirs = list[Artifact],
    non_root_module_secondary_dex_dirs = list[Artifact],
    secondary_dex_exopackage_info = [ExopackageDexInfo, None],
    proguard_text_files_path = Artifact | None,
)

ExopackageInfo = record(
    secondary_dex_info = [ExopackageDexInfo, None],
    native_library_info = [ExopackageNativeInfo, None],
    resources_info = [ExopackageResourcesInfo, None],
)

AndroidLibraryIntellijInfo = provider(
    # @unsorted-dict-items
    doc = "Information about android library that is required for Intellij project generation",
    fields = {
        "android_resource_deps": provider_field(typing.Any, default = None),  # ["AndroidResourceInfo"]
        "dummy_r_dot_java": provider_field(Artifact | None, default = None),
    },
)

AndroidResourceRDotInfo = provider(
    # @unsorted-dict-items
    doc = "Exposes the compiled R.jar for android_prebuilt_aar and android_resource targets",
    fields = {
        "dummy_r_dot_java": provider_field(Artifact | None, default = None),
    },
)

def merge_android_packageable_info(
    label: Label,
    actions: AnalysisActions,
    deps: list[Dependency],
    build_config_info: [AndroidBuildConfigInfo, None] = None,
    manifest: Artifact | None = None,
    prebuilt_native_library_dir: [PrebuiltNativeLibraryDir, None] = None,
    resource_info: [AndroidResourceInfo, None] = None,
    for_primary_apk: bool = False,
) -> AndroidPackageableInfo:
    android_packageable_deps = filter(None, [x.get(AndroidPackageableInfo) for x in deps])

    build_config_infos = _get_transitive_set(
        actions,
        filter(None, [dep.build_config_infos for dep in android_packageable_deps]),
        build_config_info,
        AndroidBuildConfigInfoTSet,
    )

    deps = _get_transitive_set(
        actions,
        filter(None, [dep.deps for dep in android_packageable_deps]),
        DepsInfo(
            name = label.raw_target(),
            deps = [dep.target_label for dep in android_packageable_deps],
            for_primary_apk = for_primary_apk,
        ),
        AndroidDepsTSet,
    )

    manifests = _get_transitive_set(
        actions,
        filter(None, [dep.manifests for dep in android_packageable_deps]),
        ManifestInfo(
            target_label = label.raw_target(),
            manifest = manifest,
        )
        if manifest
        else None,
        ManifestTSet,
    )

    prebuilt_native_library_dirs = _get_transitive_set(
        actions,
        filter(None, [dep.prebuilt_native_library_dirs for dep in android_packageable_deps]),
        prebuilt_native_library_dir,
        PrebuiltNativeLibraryDirTSet,
    )

    resource_infos = _get_transitive_set(
        actions,
        filter(None, [dep.resource_infos for dep in android_packageable_deps]),
        resource_info,
        ResourceInfoTSet,
    )

    return AndroidPackageableInfo(
        target_label = label.raw_target(),
        build_config_infos = build_config_infos,
        deps = deps,
        manifests = manifests,
        prebuilt_native_library_dirs = prebuilt_native_library_dirs,
        resource_infos = resource_infos,
    )

def _get_transitive_set(
    actions: AnalysisActions, children: list[TransitiveSet], node: typing.Any, transitive_set_definition: TransitiveSetDefinition
) -> [TransitiveSet, None]:
    kwargs = {}
    if children:
        kwargs["children"] = children
    if node:
        kwargs["value"] = node

    return actions.tset(transitive_set_definition, **kwargs) if kwargs else None

def merge_exported_android_resource_info(exported_deps: list[Dependency]) -> ExportedAndroidResourceInfo:
    exported_android_resource_infos = []
    for exported_dep in exported_deps:
        exported_resource_info = exported_dep.get(ExportedAndroidResourceInfo)
        if exported_resource_info:
            exported_android_resource_infos += exported_resource_info.resource_infos

        android_resource = exported_dep.get(AndroidResourceInfo)
        if android_resource:
            exported_android_resource_infos.append(android_resource)

    return ExportedAndroidResourceInfo(resource_infos = dedupe(exported_android_resource_infos))

def get_all_android_packageable_targets(deps: list[Dependency]) -> list[TargetLabel]:
    android_packageable_infos = filter(None, [dep.get(AndroidPackageableInfo) for dep in deps])
    android_deps_tsets = filter(None, [dep.deps for dep in android_packageable_infos])
    return [deps_info.name for android_deps_tset in android_deps_tsets for deps_info in android_deps_tset.traverse()]
