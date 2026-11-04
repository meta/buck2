# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load(
    "@prelude//android:android_providers.bzl",
    "AndroidResourceInfo",
    "AndroidResourceRDotInfo",
    "PrebuiltNativeLibraryDir",
    "RESOURCE_PRIORITY_LOW",
    "merge_android_packageable_info",
)
load("@prelude//android:android_resource.bzl", "aapt2_compile", "extract_package_from_manifest")
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:build_only_native_code.bzl", "is_build_only_native_code")
load("@prelude//android:r_dot_java.bzl", "get_dummy_r_dot_java")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load(
    "@prelude//java:java_providers.bzl",
    "ClasspathSnapshotGranularity",
    "JavaClasspathEntry",
    "create_abi",
    "create_java_library_providers",
    "generate_java_classpath_snapshot",
)
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")
load("@prelude//jvm:attrs.bzl", "jvm_common", "min_sdk_version")

def android_prebuilt_aar_impl(ctx: AnalysisContext) -> list[Provider]:
    manifest = ctx.actions.declare_output("AndroidManifest.xml", has_content_based_path = True)
    all_classes_jar = ctx.actions.declare_output("classes.jar", has_content_based_path = True)
    r_dot_txt = ctx.actions.declare_output("R.txt", has_content_based_path = True)
    res = ctx.actions.declare_output("res", dir = True, has_content_based_path = True)
    assets = ctx.actions.declare_output("assets", dir = True, has_content_based_path = True)
    jni = ctx.actions.declare_output("jni", dir = True, has_content_based_path = True)
    annotation_jars_dir = ctx.actions.declare_output("annotation_jars", dir = True, has_content_based_path = True)
    proguard_config = ctx.actions.declare_output("proguard.txt", has_content_based_path = True)
    lint_jar = ctx.actions.declare_output("lint.jar", has_content_based_path = True)

    android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo]
    unpack_aar_tool = android_toolchain.unpack_aar[RunInfo]
    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    jar_builder_tool = cmd_args(java_toolchain.jar_builder, delimiter = " ")

    unpack_aar_cmd = [
        unpack_aar_tool,
        "--aar",
        ctx.attrs.aar,
        "--manifest-path",
        manifest.as_output(),
        "--all-classes-jar-path",
        all_classes_jar.as_output(),
        "--r-dot-txt-path",
        r_dot_txt.as_output(),
        "--res-path",
        res.as_output(),
        "--assets-path",
        assets.as_output(),
        "--jni-path",
        jni.as_output(),
        "--annotation-jars-dir",
        annotation_jars_dir.as_output(),
        "--proguard-config-path",
        proguard_config.as_output(),
        "--jar-builder-tool",
        jar_builder_tool,
        "--lint-jar-path",
        lint_jar.as_output(),
    ]

    ctx.actions.run(unpack_aar_cmd, category = "android_unpack_aar")

    resource_info = AndroidResourceInfo(
        raw_target = ctx.label.raw_target(),
        aapt2_compile_output = aapt2_compile(ctx, res, android_toolchain),
        allow_strings_as_assets_resource_filtering = True,
        assets = [assets],
        manifest_file = manifest,
        r_dot_java_package = extract_package_from_manifest(ctx, manifest),
        res = res,
        res_priority = RESOURCE_PRIORITY_LOW,
        text_symbols = r_dot_txt,
        unused_resource_dep_validation_has_non_xml_resources = True,
    )

    dummy_r_dot_java_info = get_dummy_r_dot_java(
        ctx,
        android_toolchain.merge_android_resources[RunInfo],
        [resource_info],
    )

    android_resource_r_dot_info = AndroidResourceRDotInfo(
        dummy_r_dot_java = dummy_r_dot_java_info.library_output.abi,
    )

    abi = None if java_toolchain.is_bootstrap_toolchain else create_abi(ctx.actions, java_toolchain.class_abi_generator, all_classes_jar)
    abi_jar_snapshot = generate_java_classpath_snapshot(
        ctx.actions, java_toolchain.cp_snapshot_generator, ClasspathSnapshotGranularity("CLASS_LEVEL"), abi or all_classes_jar, ""
    )

    library_output_classpath_entry = JavaClasspathEntry(
        full_library = all_classes_jar,
        abi = abi or all_classes_jar,
        abi_as_dir = None,
        required_for_source_only_abi = ctx.attrs.required_for_source_only_abi,
        abi_jar_snapshot = abi_jar_snapshot,
    )

    (
        java_library_info,
        java_packaging_info,
        global_code_info,
        shared_library_info,
        linkable_graph,
        cxx_resource_info,
        template_placeholder_info,
        java_library_intellij_info,
    ) = create_java_library_providers(
        ctx = ctx,
        library_output = library_output_classpath_entry,
        global_code_config = java_toolchain.global_code_config,
        exported_deps = ctx.attrs.deps,
        provided_deps = ctx.attrs.desugar_deps,
        is_prebuilt_jar = True,
        annotation_jars_dir = annotation_jars_dir,
        proguard_config = proguard_config,
        lint_jar = lint_jar,
        sources_jar = ctx.attrs.source_jar,
        dex_weight_factor = ctx.attrs.dex_weight_factor,
    )

    native_library = PrebuiltNativeLibraryDir(
        raw_target = ctx.label.raw_target(),
        dir = jni,
        for_primary_apk = ctx.attrs.use_system_library_loader,
        is_asset = False,
    )

    return [
        java_library_info,
        java_packaging_info,
        global_code_info,
        shared_library_info,
        cxx_resource_info,
        linkable_graph,
        template_placeholder_info,
        java_library_intellij_info,
        android_resource_r_dot_info,
        merge_android_packageable_info(
            ctx.label,
            ctx.actions,
            ctx.attrs.deps,
            manifest = manifest,
            prebuilt_native_library_dir = native_library,
            resource_info = resource_info,
            for_primary_apk = ctx.attrs.for_primary_apk,
        ),
        resource_info,
        DefaultInfo(
            default_output = all_classes_jar,
            other_outputs = [
                manifest,
                r_dot_txt,
                res,
                assets,
                jni,
                annotation_jars_dir,
                proguard_config,
            ],
        ),
    ]

android_prebuilt_aar = prelude_rule(
    name = "android_prebuilt_aar",
    impl = android_prebuilt_aar_impl,
    docs = """
        An `android_prebuilt_aar()` rule takes an `.aar` file and
        makes it available as an Android dependency. As expected,
        an `android_binary()` that transitively depends on
        an `android_prebuilt_aar()` will include its contents in the generated APK.

        See the [official Android documentation](https://developer.android.com/studio/projects/android-library#aar-contents) for details about the `.aar` format.
    """,
    examples = """
        ```
        android_prebuilt_aar(
          name = 'play-services',
          aar = 'play-services-4.0.30.aar',
          source_jar = 'play-services-4.0.30-sources.jar',
          javadoc_url = 'file:///opt/android-sdk/extras/google/google_play_services/docs/reference',
        )

        android_library(
          name = 'lib',
          # This Java code can compile against Play services and reference its resources.
          srcs = glob(['*.java']),
          deps = [ ':play-services' ],
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "aar": attrs.source(
                doc = """
                Path to the `.aar` file. This may also be a build target to
                 a rule (such as a `genrule()`) whose output is
                 an `.aar` file.
            """,
            ),
            "source_jar": attrs.option(
                attrs.source(),
                default = None,
                doc = """
                Path to a JAR file that contains the `.java` files to create
                 the `.class` in the `aar`. This is frequently
                 provided for debugging purposes.
            """,
            ),
            "javadoc_url": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                URL to the Javadoc for the `.class` files in the
                 `aar`.
            """,
            ),
            "use_system_library_loader": attrs.bool(
                default = False,
                doc = """
                If this `.aar` file contains native prebuilt `.so` libraries and the
                 Java code uses these libraries via a call to `System.loadLibrary()`, then many
                 optimizations—such as exopackage, compression, or asset packaging—may not be compatible with these prebuilt libs.
                 Setting this parameter to `True` causes all of these optimizations to skip the prebuilt `.so`
                 files originating from this `.aar` file. The `.so` files will always be packaged directly into
                 the main `.apk`.
            """,
            ),
            "deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                Dependencies of this prebuilt AAR. Listed deps are automatically exported to consumers of this rule
                 and are used to resolve the transitive classpath for desugaring. Use this to specify libraries that
                 the `.aar` depends on at compile time or runtime but are not bundled within the `.aar` itself.
            """,
            ),
            "desugar_deps": attrs.list(attrs.dep(), default = []),
            "dex_weight_factor": attrs.int(default = 1),
            "for_primary_apk": attrs.bool(default = False),
            "maven_coords": attrs.option(attrs.string(), default = None),
            "min_sdk_version": attrs.default_only(attrs.option(attrs.int(), default = min_sdk_version())),
            "required_for_source_only_abi": attrs.bool(default = True),
            "_android_toolchain": toolchains_common.android(),
            "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
            "_dex_toolchain": toolchains_common.dex(),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_is_building_android_binary": attrs.default_only(attrs.bool(default = False)),
            "_java_toolchain": toolchains_common.java_for_android(),
        }
        | jvm_common.classic_java_content_based_paths()
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
