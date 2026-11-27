# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//android:android_toolchain.bzl", "AidlToolchainInfo", "AndroidPlatformInfo", "AndroidToolchainInfo")
load("@prelude//java:java_providers.bzl", "ClasspathSnapshotGranularity", "generate_java_classpath_snapshot")
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")
load("@prelude//target_stats:target_stats_tools.bzl", "TargetStatsToolsInfo")
load("@prelude//tests:test_listing.bzl", "TestListingInfo")

def _android_sdk_tools_impl(ctx):
    sub_targets = {}
    sdk_path = ctx.attrs.android_sdk_path
    build_tools = "build-tools/{}".format(ctx.attrs.build_tools_version)
    platform = "platforms/{}".format(ctx.attrs.compile_sdk_version)

    # Every SDK file the build uses goes through `check_sdk_files`, so a wrong `[android] sdk_path`
    # or a missing SDK package fails with a message naming the fix instead of a dangling path.
    def check(name, files, link):
        output = ctx.actions.declare_output(name, has_content_based_path = False)
        args = cmd_args(ctx.attrs._check_sdk_files[RunInfo], "--sdk-path", sdk_path)
        for relative_path, package in files:
            args.add("--file", relative_path, package)
        if link:
            args.add("--link")
        args.add("--output", output.as_output())
        ctx.actions.run(args, category = "android_sdk_check", identifier = name)
        return output

    build_tools_package = "build-tools;{}".format(ctx.attrs.build_tools_version)
    build_tools_checked = check(
        "build_tools_checked",
        [("{}/{}".format(build_tools, tool), build_tools_package) for tool in ["aapt2", "aidl", "zipalign"]],
        link = False,
    )
    for tool in ["aapt2", "aidl", "zipalign"]:
        sub_targets[tool] = [RunInfo(args = cmd_args("{}/{}/{}".format(sdk_path, build_tools, tool), hidden = build_tools_checked))]

    adb_checked = check("adb_checked", [("platform-tools/adb", "platform-tools")], link = False)
    sub_targets["adb"] = [RunInfo(args = cmd_args("{}/platform-tools/adb".format(sdk_path), hidden = adb_checked))]

    platform_package = "platforms;{}".format(ctx.attrs.compile_sdk_version)
    for file_name in ["android.jar", "core-for-system-modules.jar", "framework.aidl"]:
        linked = check(file_name, [("{}/{}".format(platform, file_name), platform_package)], link = True)
        sub_targets[file_name] = [DefaultInfo(default_output = linked)]

    optimized_proguard_config = ctx.actions.declare_output("proguard-android-optimize.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["ln", "-s", "{}/tools/proguard/proguard-android-optimize.txt".format(ctx.attrs.android_sdk_path), optimized_proguard_config.as_output()]),
        category = "optimized_proguard_config_symlink",
    )
    sub_targets["optimized_proguard_config"] = [DefaultInfo(default_output = optimized_proguard_config)]

    proguard_config = ctx.actions.declare_output("proguard-android.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["ln", "-s", "{}/tools/proguard/proguard-android.txt".format(ctx.attrs.android_sdk_path), proguard_config.as_output()]),
        category = "proguard_config_symlink",
    )
    sub_targets["proguard_config"] = [DefaultInfo(default_output = proguard_config)]

    proguard_jar = ctx.actions.declare_output("proguard.jar", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["ln", "-s", "{}/tools/proguard/lib/proguard.jar".format(ctx.attrs.android_sdk_path), proguard_jar.as_output()]),
        category = "proguard_jar_symlink",
    )
    sub_targets["proguard.jar"] = [DefaultInfo(default_output = proguard_jar)]

    return [
        DefaultInfo(sub_targets = sub_targets),
    ]

android_sdk_tools = rule(
    impl = _android_sdk_tools_impl,
    attrs = {
        "android_sdk_path": attrs.string(default = read_root_config("android", "sdk_path", "/opt/android_sdk")),
        "build_tools_version": attrs.string(default = read_root_config("android", "build_tools_version", "35.0.0")),
        "compile_sdk_version": attrs.string(default = read_root_config("android", "compile_sdk_version", "android-35")),
        "_check_sdk_files": attrs.exec_dep(default = "prelude//android/tools:check_sdk_files", providers = [RunInfo]),
    },
)

def system_android_aidl_toolchain(name, android_sdk_tools_target, **kwargs):
    kwargs["aidl"] = "{}[aidl]".format(android_sdk_tools_target)
    kwargs["framework_aidl_file"] = "{}[framework.aidl]".format(android_sdk_tools_target)
    _system_android_aidl_toolchain_rule(name = name, **kwargs)

def _system_android_aidl_toolchain_rule_impl(ctx):
    return [
        DefaultInfo(),
        AidlToolchainInfo(
            aidl = ctx.attrs.aidl[RunInfo],
            framework_aidl_file = ctx.attrs.framework_aidl_file,
        ),
    ]

_system_android_aidl_toolchain_rule = rule(
    attrs = {
        "aidl": attrs.exec_dep(providers = [RunInfo]),
        "framework_aidl_file": attrs.source(),
    },
    impl = _system_android_aidl_toolchain_rule_impl,
    is_toolchain_rule = True,
)

def system_android_toolchain(name, android_sdk_tools_target, jdk_system_image, **kwargs):
    kwargs["aapt2_filter_resources"] = "prelude//android/tools:filter_extra_resources"
    kwargs["aapt2"] = "{}[aapt2]".format(android_sdk_tools_target)
    kwargs["aar_builder"] = "prelude//toolchains/android/src/com/facebook/buck/android/aar:aar_builder_binary"
    kwargs["adb"] = "{}[adb]".format(android_sdk_tools_target)
    kwargs["android_jar"] = "{}[android.jar]".format(android_sdk_tools_target)
    kwargs["android_optional_jars"] = []
    kwargs["apk_builder"] = "prelude//toolchains/android/src/com/facebook/buck/android/apk:apk_builder_binary"
    kwargs["apk_module_graph"] = "prelude//toolchains/android/src/com/facebook/buck/android/apkmodule:apkmodule_binary"
    kwargs["app_without_resources_stub"] = "prelude//android/tools:app_without_resources_stub"
    kwargs["bundle_apks_builder"] = "prelude//toolchains/android/src/com/facebook/buck/android/bundle:bundle_apks_builder_binary"
    kwargs["bundle_builder"] = "prelude//toolchains/android/src/com/facebook/buck/android/bundle:bundle_builder_binary"
    kwargs["combine_native_library_dirs"] = "prelude//android/tools:combine_native_library_dirs"
    kwargs["consolidate_class_names"] = "prelude//android/tools:consolidate_class_names"
    kwargs["copy_string_resources"] = "prelude//toolchains/android/src/com/facebook/buck/android/resources/strings:copy_string_resources_binary"
    kwargs["cross_module_native_deps_check"] = True
    kwargs["d8_command"] = "prelude//toolchains/android/src/com/facebook/buck/android/dex:run_d8_binary"
    kwargs["duplicate_class_checker"] = "prelude//android/tools:duplicate_class_checker"
    kwargs["exo_resources_rewriter"] = "prelude//toolchains/android/src/com/facebook/buck/android/resources:exo_resources_rewriter_binary"
    kwargs["filter_dex_class_names"] = "prelude//android/tools:filter_dex"
    kwargs["filter_prebuilt_native_library_dir"] = "prelude//android/tools:filter_prebuilt_native_library_dir"
    kwargs["filter_resources"] = "prelude//toolchains/android/src/com/facebook/buck/android/resources/filter:filter_resources_binary"
    # @oss-disable[end= ]: kwargs["gatorade_mergemap_tool"] = "prelude//android/tools/meta_only:gatorade_mergemap_tool"
    kwargs["generate_build_config"] = "prelude//toolchains/android/src/com/facebook/buck/android/build_config:generate_build_config_binary"
    kwargs["generate_manifest"] = "prelude//toolchains/android/src/com/facebook/buck/android/manifest:generate_manifest_binary"
    kwargs["sort_pre_dexed_files"] = "prelude//android/tools:sort_pre_dexed_files"
    kwargs["installer"] = "prelude//toolchains/android/installer:android_installer_with_buckconfig"
    kwargs["instrumentation_test_can_run_locally"] = True
    kwargs["collect_perfetto"] = False
    kwargs["instrumentation_test_runner_classpath"] = [
        "prelude//toolchains/android/src/com/facebook/buck/testrunner:testrunner-bin-android-fixed",
        "prelude//toolchains/android/third-party:android-common",
        "prelude//toolchains/android/third-party:ddmlib",
        "prelude//toolchains/android/third-party:guava-jar",
        "prelude//toolchains/android/third-party:failureaccess",
        "prelude//toolchains/android/third-party:listenablefuture",
        "prelude//toolchains/android/third-party:kxml2",
        "prelude//toolchains/android/third-party:protobuf",
        "prelude//toolchains/android/third-party:jackson-annotations",
        "prelude//toolchains/android/third-party:jackson-core",
        "prelude//toolchains/android/third-party:jackson-databind-jar",
    ]
    kwargs["instrumentation_test_runner_main_class"] = "com.facebook.buck.testrunner.InstrumentationMain"
    kwargs["jar_splitter_command"] = "prelude//toolchains/android/src/com/facebook/buck/android/dex:jar_splitter_binary"
    kwargs["jdk_system_image"] = jdk_system_image
    kwargs["manifest_utils"] = "prelude//toolchains/android/src/com/facebook/buck/android:manifest_utils_binary"
    kwargs["merge_android_resource_sources"] = "prelude//toolchains/android/src/com/facebook/buck/android/aapt:merge_android_resource_sources_binary"
    kwargs["merge_android_resources"] = "prelude//toolchains/android/src/com/facebook/buck/android/resources:merge_android_resources_binary"
    kwargs["merge_assets"] = "prelude//toolchains/android/src/com/facebook/buck/android/resources:merge_assets_binary"
    kwargs["mergemap_tool"] = "prelude//android/tools:compute_merge_sequence"
    kwargs["mini_aapt"] = "prelude//toolchains/android/src/com/facebook/buck/android/aapt:mini_aapt_binary"
    kwargs["multi_dex_command"] = "prelude//toolchains/android/src/com/facebook/buck/android/dex:multi_dex_binary"
    kwargs["native_libs_as_assets_metadata"] = "prelude//android/tools:native_libs_as_assets_metadata"
    # Android SDKs no longer ship tools/proguard: ProGuard and its default configs come from Maven.
    kwargs["optimized_proguard_config"] = "prelude//toolchains/android/third-party/proguard:optimized_proguard_config"
    kwargs["package_meta_inf_version_files"] = False
    kwargs["package_strings_as_assets"] = "prelude//toolchains/android/src/com/facebook/buck/android/resources/strings:package_strings_as_assets_binary"
    kwargs["proguard_config"] = "prelude//toolchains/android/third-party/proguard:proguard_config"
    kwargs["proguard_jar"] = "prelude//toolchains/android/third-party/proguard:proguard"
    kwargs["r_dot_java_weight_factor"] = int(read_root_config("android", "r_dot_java_weight_factor", "8"))
    # @oss-disable[end= ]: kwargs["redex_native_class_names"] = "prelude//android/tools/meta_only:redex_native_class_names"
    kwargs["replace_application_id_placeholders"] = (
        "prelude//toolchains/android/src/com/facebook/buck/android/manifest:replace_application_id_placeholders_binary"
    )
    kwargs["secondary_dex_compression_command"] = "prelude//toolchains/android/src/com/facebook/buck/android/dex:secondary_dex_compression_binary"
    # Bytes of estimated dex weight per secondary dex. Meta's toolchain uses 12 MiB; the old
    # 1024 made every few classes their own dex (plus one canary class compile each).
    kwargs["secondary_dex_weight_limit"] = int(read_root_config("android", "secondary_dex_weight_limit", str(12 * 1024 * 1024)))
    kwargs["unpack_aar"] = "prelude//android/tools:unpack_aar"
    kwargs["zipalign"] = "{}[zipalign]".format(android_sdk_tools_target)

    system_android_toolchain_rule(name = name, **kwargs)

def system_android_toolchain_rule_impl(ctx):
    # Kotlin incremental compilation needs a classpath snapshot of android.jar too.
    bootclasspath_snapshots = [
        generate_java_classpath_snapshot(
            ctx.actions,
            ctx.attrs._java_toolchain[JavaToolchainInfo].cp_snapshot_generator,
            ClasspathSnapshotGranularity("CLASS_LEVEL"),
            ctx.attrs.android_jar,
            "",
        ),
    ]
    bootclasspath_snapshots = [x for x in bootclasspath_snapshots if x != None]
    return [
        DefaultInfo(),
        AndroidPlatformInfo(
            name = ctx.attrs.name,
        ),
        TestListingInfo(
            list_tests = ctx.attrs.list_tests,
        ),
        AndroidToolchainInfo(
            target_stats_tools = ctx.attrs.target_stats_tools[TargetStatsToolsInfo] if ctx.attrs.target_stats_tools else None,
            aapt2 = ctx.attrs.aapt2[RunInfo],
            aapt2_filter_resources = ctx.attrs.aapt2_filter_resources[RunInfo],
            aar_builder = ctx.attrs.aar_builder[RunInfo],
            adb = ctx.attrs.adb[RunInfo],
            android_bootclasspath = [ctx.attrs.android_jar],
            android_bootclasspath_snapshots = bootclasspath_snapshots,
            android_jar = ctx.attrs.android_jar,
            android_optional_jars = ctx.attrs.android_optional_jars,
            apk_builder = ctx.attrs.apk_builder,
            apk_module_graph = ctx.attrs.apk_module_graph,
            app_without_resources_stub = ctx.attrs.app_without_resources_stub,
            bundle_apks_builder = ctx.attrs.bundle_apks_builder,
            bundle_builder = ctx.attrs.bundle_builder,
            combine_native_library_dirs = ctx.attrs.combine_native_library_dirs,
            consolidate_class_names = ctx.attrs.consolidate_class_names,
            copy_string_resources = ctx.attrs.copy_string_resources,
            cross_module_native_deps_check = ctx.attrs.cross_module_native_deps_check,
            d8_command = ctx.attrs.d8_command,
            duplicate_class_checker = ctx.attrs.duplicate_class_checker,
            exo_resources_rewriter = ctx.attrs.exo_resources_rewriter,
            filter_dex_class_names = ctx.attrs.filter_dex_class_names,
            filter_prebuilt_native_library_dir = ctx.attrs.filter_prebuilt_native_library_dir,
            filter_resources = ctx.attrs.filter_resources,
            # @oss-disable[end= ]: gatorade_mergemap_tool = ctx.attrs.gatorade_mergemap_tool[RunInfo],
            generate_build_config = ctx.attrs.generate_build_config,
            generate_manifest = ctx.attrs.generate_manifest,
            installer = ctx.attrs.installer,
            instrumentation_test_can_run_locally = ctx.attrs.instrumentation_test_can_run_locally,
            collect_perfetto = ctx.attrs.collect_perfetto,
            instrumentation_test_runner_classpath = ctx.attrs.instrumentation_test_runner_classpath,
            instrumentation_test_runner_main_class = ctx.attrs.instrumentation_test_runner_main_class,
            jar_splitter_command = ctx.attrs.jar_splitter_command,
            jdk_system_image = ctx.attrs.jdk_system_image,
            jni_onload_check = ctx.attrs.jni_onload_check,
            manifest_utils = ctx.attrs.manifest_utils,
            merge_android_resource_sources = ctx.attrs.merge_android_resource_sources,
            merge_android_resources = ctx.attrs.merge_android_resources,
            merge_assets = ctx.attrs.merge_assets,
            mergemap_tool = ctx.attrs.mergemap_tool[RunInfo],
            mini_aapt = ctx.attrs.mini_aapt,
            multi_dex_command = ctx.attrs.multi_dex_command,
            native_libs_as_assets_metadata = ctx.attrs.native_libs_as_assets_metadata,
            optimized_proguard_config = ctx.attrs.optimized_proguard_config,
            p7zip = None,
            package_meta_inf_version_files = ctx.attrs.package_meta_inf_version_files,
            package_strings_as_assets = ctx.attrs.package_strings_as_assets,
            proguard_config = ctx.attrs.proguard_config,
            proguard_jar = ctx.attrs.proguard_jar,
            r_dot_java_weight_factor = ctx.attrs.r_dot_java_weight_factor,
            # @oss-disable[end= ]: redex_native_class_names = ctx.attrs.redex_native_class_names[RunInfo] if ctx.attrs.redex_native_class_names else None,
            replace_application_id_placeholders = ctx.attrs.replace_application_id_placeholders,
            secondary_dex_compression_command = ctx.attrs.secondary_dex_compression_command,
            secondary_dex_weight_limit = ctx.attrs.secondary_dex_weight_limit,
            sort_pre_dexed_files = ctx.attrs.sort_pre_dexed_files,
            unpack_aar = ctx.attrs.unpack_aar,
            zipalign = ctx.attrs.zipalign,
        ),
    ]

system_android_toolchain_rule = rule(
    # The tools run on the execution platform, so they are built once for it, not once for each
    # target configuration (an `android_binary` and its CPU splits each have their own).
    attrs = {
        "aapt2": attrs.exec_dep(providers = [RunInfo]),
        "aapt2_filter_resources": attrs.exec_dep(providers = [RunInfo]),
        "aar_builder": attrs.exec_dep(providers = [RunInfo]),
        "adb": attrs.exec_dep(providers = [RunInfo]),
        "android_jar": attrs.source(),
        "android_optional_jars": attrs.list(attrs.source()),
        "apk_builder": attrs.exec_dep(providers = [RunInfo]),
        "apk_module_graph": attrs.exec_dep(providers = [RunInfo]),
        "app_without_resources_stub": attrs.source(),
        "bundle_apks_builder": attrs.exec_dep(providers = [RunInfo]),
        "bundle_builder": attrs.exec_dep(providers = [RunInfo]),
        "collect_perfetto": attrs.bool(),
        "combine_native_library_dirs": attrs.exec_dep(providers = [RunInfo]),
        "consolidate_class_names": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "copy_string_resources": attrs.exec_dep(providers = [RunInfo]),
        "cross_module_native_deps_check": attrs.bool(),
        "d8_command": attrs.exec_dep(providers = [RunInfo]),
        "duplicate_class_checker": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "exo_resources_rewriter": attrs.exec_dep(providers = [RunInfo]),
        "filter_dex_class_names": attrs.exec_dep(providers = [RunInfo]),
        "filter_prebuilt_native_library_dir": attrs.exec_dep(providers = [RunInfo]),
        "filter_resources": attrs.exec_dep(providers = [RunInfo]),
        # @oss-disable[end= ]: "gatorade_mergemap_tool": attrs.exec_dep(providers = [RunInfo]),
        "generate_build_config": attrs.exec_dep(providers = [RunInfo]),
        "generate_manifest": attrs.exec_dep(providers = [RunInfo]),
        "installer": attrs.label(),
        "instrumentation_test_can_run_locally": attrs.bool(),
        "instrumentation_test_runner_classpath": attrs.list(attrs.source()),
        "instrumentation_test_runner_main_class": attrs.string(),
        "jar_splitter_command": attrs.exec_dep(providers = [RunInfo]),
        "jdk_system_image": attrs.source(),
        "jni_onload_check": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "list_tests": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "manifest_utils": attrs.exec_dep(providers = [RunInfo]),
        "merge_android_resource_sources": attrs.exec_dep(providers = [RunInfo]),
        "merge_android_resources": attrs.exec_dep(providers = [RunInfo]),
        "merge_assets": attrs.exec_dep(providers = [RunInfo]),
        "mergemap_tool": attrs.exec_dep(providers = [RunInfo]),
        "mini_aapt": attrs.exec_dep(providers = [RunInfo]),
        "multi_dex_command": attrs.exec_dep(providers = [RunInfo]),
        "native_libs_as_assets_metadata": attrs.exec_dep(providers = [RunInfo]),
        "optimized_proguard_config": attrs.source(),
        "package_meta_inf_version_files": attrs.bool(),
        "package_strings_as_assets": attrs.exec_dep(providers = [RunInfo]),
        "proguard_config": attrs.source(),
        "proguard_jar": attrs.source(),
        "r_dot_java_weight_factor": attrs.int(),
        # @oss-disable[end= ]: "redex_native_class_names": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "replace_application_id_placeholders": attrs.exec_dep(providers = [RunInfo]),
        "secondary_dex_compression_command": attrs.exec_dep(providers = [RunInfo]),
        "secondary_dex_weight_limit": attrs.int(),
        "sort_pre_dexed_files": attrs.exec_dep(providers = [RunInfo]),
        "target_stats_tools": attrs.option(attrs.exec_dep(providers = [TargetStatsToolsInfo]), default = None),
        "unpack_aar": attrs.exec_dep(providers = [RunInfo]),
        "zipalign": attrs.exec_dep(providers = [RunInfo]),
        "_java_toolchain": attrs.toolchain_dep(default = "toolchains//:java", providers = [JavaToolchainInfo]),
    },
    impl = system_android_toolchain_rule_impl,
    is_toolchain_rule = True,
)

_NDK_TARGETS = {
    "config//cpu/constraints:arm32": "armv7a-linux-androideabi",
    "config//cpu/constraints:arm64": "aarch64-linux-android",
    "config//cpu/constraints:x86_32": "i686-linux-android",
    "config//cpu/constraints:x86_64": "x86_64-linux-android",
}

def _ndk_host_tag() -> str:
    if host_info().os.is_macos:
        return "darwin-x86_64"
    if host_info().os.is_windows:
        return "windows-x86_64"
    return "linux-x86_64"

def android_ndk_cxx_toolchain_attrs(ndk_path: str, api_level: int = 21) -> dict[str, typing.Any]:
    """
    Attributes for `system_cxx_toolchain` that compile and link with the Android NDK at
    `ndk_path` for each CPU that an `android_binary` builds native code for (its
    `cpu_filters`), and leave the host toolchain for everything else.

    `api_level` is the minimum Android API level of the native code, as `minSdkVersion`.
    """
    bin = "{}/toolchains/llvm/prebuilt/{}/bin/".format(ndk_path, _ndk_host_tag())

    def android_only(value, host_value = None):
        return select({
            "DEFAULT": host_value,
            "prelude//os:building_android_binary": value,
        })

    def target_flags(extra_flags):
        return android_only(
            select({cpu: ["--target={}{}".format(target, api_level)] + extra_flags for cpu, target in _NDK_TARGETS.items()}),
            [],
        )

    return {
        "archiver": android_only(bin + "llvm-ar"),
        "c_flags": target_flags(["-DANDROID"]),
        "compiler": android_only(bin + "clang"),
        "compiler_type": android_only("clang"),
        "cxx_compiler": android_only(bin + "clang++"),
        "cxx_flags": target_flags(["-DANDROID"]),
        "link_flags": target_flags([
            "-static-libstdc++",
            "-Wl,--build-id=sha1",
            "-Wl,--no-undefined",
            # 16 KB pages, which Google Play requires for apps targeting Android 15 and later.
            "-Wl,-z,max-page-size=16384",
        ]),
        "linker": android_only(bin + "clang++"),
        "nm": android_only(bin + "llvm-nm"),
        "objcopy": android_only(bin + "llvm-objcopy"),
        "objdump": android_only(bin + "llvm-objdump"),
        "ranlib": android_only(bin + "llvm-ranlib"),
        "strip": android_only(bin + "llvm-strip"),
    }
