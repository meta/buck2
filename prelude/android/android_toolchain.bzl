# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

AndroidPlatformInfo = provider(
    fields = {
        "name": provider_field(typing.Any, default = None),
    }
)

AidlToolchainInfo = provider(
    fields = {
        "aidl": provider_field(typing.Any, default = None),
        "framework_aidl_file": provider_field(typing.Any, default = None),
    }
)

AndroidToolchainInfo = provider(
    fields = {
        "aapt2": provider_field(typing.Any, default = None),
        "aapt2_filter_resources": provider_field(typing.Any, default = None),
        "aar_builder": provider_field(typing.Any, default = None),
        "adb": provider_field(typing.Any, default = None),
        "android_bootclasspath": provider_field(typing.Any, default = None),
        "android_bootclasspath_snapshots": provider_field(typing.Any, default = []),
        "android_error_handler": provider_field(typing.Any, default = None),
        "android_jar": provider_field(typing.Any, default = None),
        "android_optional_jars": provider_field(typing.Any, default = None),
        "apk_builder": provider_field(typing.Any, default = None),
        # Threads that deflate APK files when `apk_builder_fast_packaging` is set.
        "apk_builder_compression_threads": provider_field(int, default = 4),
        # Package APKs with `FastApkJarBuilder`: copy compressed entries of zip inputs as they are,
        # and deflate files on `apk_builder_compression_threads` threads.
        "apk_builder_fast_packaging": provider_field(bool, default = False),
        "apk_module_graph": provider_field(typing.Any, default = None),
        # `always` signs APKs with v1 and v2 signatures; `min-sdk` adds the v1 signature only when
        # the APK's minSdkVersion is below 24.
        "apk_v1_signing_policy": provider_field(str, default = "always"),
        "app_without_resources_stub": provider_field(typing.Any, default = None),
        "build_info_generator": provider_field(typing.Any, default = None),
        "bundle_apks_builder": provider_field(typing.Any, default = None),
        "bundle_builder": provider_field(typing.Any, default = None),
        "collect_perfetto": provider_field(typing.Any, default = None),
        "combine_native_library_dirs": provider_field(typing.Any, default = None),
        "consolidate_class_names": provider_field(typing.Any, default = None),
        "copy_string_resources": provider_field(typing.Any, default = None),
        "cross_module_native_deps_check": provider_field(typing.Any, default = None),
        "d8_command": provider_field(typing.Any, default = None),
        "duplicate_class_checker": provider_field(typing.Any, default = None),
        "exo_resources_rewriter": provider_field(typing.Any, default = None),
        "filter_dex_class_names": provider_field(typing.Any, default = None),
        "filter_prebuilt_native_library_dir": provider_field(typing.Any, default = None),
        "filter_resources": provider_field(typing.Any, default = None),
        # @oss-disable[end= ]: "gatorade_mergemap_tool": provider_field(typing.Any, default = None),
        "generate_build_config": provider_field(typing.Any, default = None),
        "generate_manifest": provider_field(typing.Any, default = None),
        "installer": provider_field(typing.Any, default = None),
        "instrumentation_test_can_run_locally": provider_field(typing.Any, default = None),
        "instrumentation_test_runner_classpath": provider_field(typing.Any, default = None),
        "instrumentation_test_runner_main_class": provider_field(typing.Any, default = None),
        "jar_splitter_command": provider_field(typing.Any, default = None),
        "jdk_system_image": provider_field(typing.Any, default = None),
        "jni_onload_check": provider_field(typing.Any, default = None),
        "manifest_utils": provider_field(typing.Any, default = None),
        "merge_android_resource_sources": provider_field(typing.Any, default = None),
        "merge_android_resources": provider_field(typing.Any, default = None),
        "merge_assets": provider_field(typing.Any, default = None),
        "mergemap_tool": provider_field(typing.Any, default = None),
        "mini_aapt": provider_field(typing.Any, default = None),
        "multi_dex_command": provider_field(typing.Any, default = None),
        # JVM max heap (`-Xmx` value, e.g. "28g") for `multi_dex_command`. When None, the JVM picks its default.
        "multi_dex_max_heap_size": provider_field(str | None, default = None),
        "native_libs_as_assets_metadata": provider_field(typing.Any, default = None),
        "optimized_proguard_config": provider_field(typing.Any, default = None),
        "p7zip": provider_field(typing.Any, default = None),
        "package_meta_inf_version_files": provider_field(typing.Any, default = None),
        "package_strings_as_assets": provider_field(typing.Any, default = None),
        "proguard_config": provider_field(typing.Any, default = None),
        "proguard_jar": provider_field(typing.Any, default = None),
        "r_dot_java_weight_factor": provider_field(typing.Any, default = None),
        # @oss-disable[end= ]: "redex_native_class_names": provider_field(typing.Any, default = None),
        "replace_application_id_placeholders": provider_field(typing.Any, default = None),
        "secondary_dex_compression_command": provider_field(typing.Any, default = None),
        "secondary_dex_weight_limit": provider_field(typing.Any, default = None),
        "sort_pre_dexed_files": provider_field(typing.Any),
        "target_stats_tools": provider_field(typing.Any, default = None),
        "unpack_aar": provider_field(typing.Any, default = None),
        "zipalign": provider_field(typing.Any, default = None),
    },
)
