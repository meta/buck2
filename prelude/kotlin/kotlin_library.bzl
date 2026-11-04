# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:attrs_validators.bzl", "get_attrs_validation_specs", "validation_common")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//:validation_deps.bzl", "VALIDATION_DEPS_ATTR_NAME", "get_validation_deps_outputs")
load("@prelude//android:android_providers.bzl", "merge_android_packageable_info")
load("@prelude//android:build_only_native_code.bzl", "is_build_only_native_code")
load("@prelude//android:configuration.bzl", "is_building_android_binary_attr")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load(
    "@prelude//java:java_library.bzl",
    "build_java_library",
    "jvm_target_stats",
    "split_on_archives_and_plain_files",
)
load(
    "@prelude//java:java_providers.bzl",
    "JavaClasspathEntry",
    "JavaCompilingDepsTSet",  # @unused Used as a type
    "JavaLibraryInfo",
    "JavaPackagingDepTSet",
    "JavaPackagingInfo",
    "JavaProviders",
    "create_java_library_providers",
    "create_native_providers",
    "get_compiling_deps_tset",
    "to_list",
)
load(
    "@prelude//java:java_toolchain.bzl",
    "AbiGenerationMode",
    "JavaToolchainInfo",
)
load(
    "@prelude//java/plugins:java_annotation_processor.bzl",
    "AnnotationProcessorProperties",
    "create_annotation_processor_properties",
    "create_ksp_annotation_processor_properties",
)
load("@prelude//java/plugins:java_plugin.bzl", "create_plugin_params")
load("@prelude//java/utils:java_more_utils.bzl", "get_path_separator_for_exec_os")
load(
    "@prelude//java/utils:java_utils.bzl",
    "CustomJdkInfo",  # @unused Used as a type
    "derive_javac",
    "get_abi_generation_mode",
    "get_class_to_source_map_info",
    "get_default_info",
    "get_java_version_attributes",
)
load("@prelude//jvm:attrs.bzl", "jvm_common", "min_sdk_version")
load("@prelude//jvm:nullsafe.bzl", "get_nullsafe_info")
load(
    "@prelude//kotlin:kotlin_toolchain.bzl",
    "KotlinToolchainInfo",
)
load("@prelude//kotlin:kotlin_utils.bzl", "get_friend_paths", "get_kotlinc_compatible_target")
load("@prelude//kotlin:kotlincd_jar_creator.bzl", "create_jar_artifact_kotlincd")
load("@prelude//utils:argfile.bzl", "at_argfile")
load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:lazy.bzl", "lazy")
load("@prelude//utils:utils.bzl", "map_idx")
load(":attrs.bzl", "no_x_jdk_release_arg")

_JAVA_OR_KOTLIN_FILE_EXTENSION = [".java", ".kt"]

def _create_kotlin_sources(
    ctx: AnalysisContext,
    srcs: list[Artifact],
    deps: list[Dependency],
    annotation_processor_properties: AnnotationProcessorProperties,
    ksp_annotation_processor_properties: AnnotationProcessorProperties,
    additional_classpath_entries: JavaCompilingDepsTSet | None,
    bootclasspath_entries: list[Artifact],
    plugins: list[tuple],
    target_level: int,
    output_artifact_prefix: str = "",
) -> (Artifact, Artifact | None, Artifact | None):
    """
    Runs kotlinc on the provided kotlin sources.
    """

    kotlin_toolchain = ctx.attrs._kotlin_toolchain[KotlinToolchainInfo]
    compile_kotlin_tool = kotlin_toolchain.compile_kotlin[RunInfo]
    kotlinc = kotlin_toolchain.kotlinc[RunInfo]
    kotlinc_output = ctx.actions.declare_output(
        output_artifact_prefix,
        "kotlinc_classes_output",
        dir = True,
        has_content_based_path = True,
    )

    compile_kotlin_cmd_args = [
        compile_kotlin_tool,
        "--kotlinc_output",
        kotlinc_output.as_output(),
    ]
    compile_kotlin_cmd_hidden = cmd_args()

    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    zip_scrubber_args = ["--zip_scrubber", cmd_args(java_toolchain.zip_scrubber, delimiter = " ")]
    compile_kotlin_cmd_args.append(zip_scrubber_args)

    kotlinc_cmd_args = cmd_args([kotlinc])

    compiling_classpath = cmd_args()
    if additional_classpath_entries:
        compiling_classpath.add(additional_classpath_entries.project_as_args("args_for_compiling"))

    # kotlic doesn't support -bootclasspath param, so adding `bootclasspath_entries` into kotlin classpath
    compiling_classpath.add(bootclasspath_entries)

    compiling_deps_tset = get_compiling_deps_tset(ctx.actions, deps + [kotlin_toolchain.kotlin_stdlib])
    if compiling_deps_tset:
        compiling_classpath.add(compiling_deps_tset.project_as_args("args_for_compiling"))

    classpath_args = cmd_args(
        compiling_classpath,
        delimiter = get_path_separator_for_exec_os(ctx),
    )

    compile_kotlin_cmd_hidden.add(compiling_classpath)

    kotlinc_cmd_args.add(["-classpath"])
    kotlinc_cmd_args.add(
        at_argfile(
            actions = ctx.actions,
            name = "{}kotlinc_classpath".format(output_artifact_prefix),
            args = classpath_args,
            allow_args = True,
        )
    )

    # this is required for the Kotlin compiler to be able to use jspecify annotations
    kotlinc_cmd_args.add(["-Xjspecify-annotations=strict"])

    jdk_release = getattr(ctx.attrs, "jdk_release", None) or ctx.attrs.java_version
    jvm_target = None
    if jdk_release and not ctx.attrs.no_x_jdk_release:
        kotlinc_cmd_args.add(["-Xjdk-release=" + jdk_release])
    else:
        jvm_target = get_kotlinc_compatible_target(str(target_level))
        kotlinc_cmd_args.add([
            "-jvm-target",
            jvm_target,
        ])

    module_name = ctx.label.package.replace("/", ".") + "." + ctx.label.name
    kotlinc_cmd_args.add(
        [
            "-module-name",
            module_name,
            "-no-stdlib",
            "-no-reflect",
        ]
        + ctx.attrs.extra_kotlinc_arguments
        + get_language_version_arg(ctx),
    )

    kapt_generated_sources_output = None
    if annotation_processor_properties.annotation_processors:
        compile_kotlin_cmd_args.extend(
            ["--kapt_annotation_processing_jar", kotlin_toolchain.annotation_processing_jar[JavaLibraryInfo].library_output.full_library]
        )
        compile_kotlin_cmd_args.extend(
            ["--kapt_annotation_processors", ",".join([p for ap in annotation_processor_properties.annotation_processors for p in ap.processors])]
        )
        compile_kotlin_cmd_args.extend(["--kapt_annotation_processor_params", ";".join(annotation_processor_properties.annotation_processor_params)])

        annotation_processor_classpath_tsets = filter(None, ([ap.deps for ap in annotation_processor_properties.annotation_processors])) + [
            dep[JavaPackagingInfo].packaging_deps for dep in [kotlin_toolchain.annotation_processing_jar, kotlin_toolchain.kotlin_stdlib]
        ]
        annotation_processor_classpath = ctx.actions.tset(
            JavaPackagingDepTSet,
            children = annotation_processor_classpath_tsets,
        ).project_as_args("full_jar_args")
        kapt_classpath_file = ctx.actions.write(
            "{}kapt_classpath_file".format(output_artifact_prefix), annotation_processor_classpath, has_content_based_path = False
        )
        compile_kotlin_cmd_args.extend(["--kapt_classpath_file", kapt_classpath_file])
        compile_kotlin_cmd_hidden.add(annotation_processor_classpath)

        sources_output = ctx.actions.declare_output(output_artifact_prefix, "kapt_sources_output", has_content_based_path = True)
        compile_kotlin_cmd_args.append(["--kapt_sources_output", sources_output.as_output()])
        classes_output = ctx.actions.declare_output(output_artifact_prefix, "kapt_classes_output", has_content_based_path = True)
        compile_kotlin_cmd_args.append(["--kapt_classes_output", classes_output.as_output()])
        stubs = ctx.actions.declare_output(output_artifact_prefix, "kapt_stubs", has_content_based_path = True)
        compile_kotlin_cmd_args.append(["--kapt_stubs", stubs.as_output()])

        kapt_generated_sources_output = ctx.actions.declare_output(output_artifact_prefix, "kapt_generated_sources_output.src.zip", has_content_based_path = True)
        compile_kotlin_cmd_args.append(["--kapt_generated_sources_output", kapt_generated_sources_output.as_output()])
        compile_kotlin_cmd_args.append(["--kapt_base64_encoder", cmd_args(kotlin_toolchain.kapt_base64_encoder[RunInfo], delimiter = " ")])
        generated_kotlin_output = ctx.actions.declare_output(output_artifact_prefix, "kapt_generated_kotlin_output", has_content_based_path = True)
        compile_kotlin_cmd_args.append(["--kapt_generated_kotlin_output", generated_kotlin_output.as_output()])
        if jvm_target:
            compile_kotlin_cmd_args.append(["--kapt_jvm_target", jvm_target])

    friend_paths = get_friend_paths(ctx)
    if friend_paths:
        concat_friends_paths = cmd_args(
            [friend_path.library_output.abi for friend_path in map_idx(JavaLibraryInfo, friend_paths) if friend_path.library_output], delimiter = ","
        )
        kotlinc_cmd_args.add(cmd_args(["-Xfriend-paths", concat_friends_paths], delimiter = "="))

    zipped_sources, plain_sources = split_on_archives_and_plain_files(srcs, _JAVA_OR_KOTLIN_FILE_EXTENSION)

    kotlinc_cmd_args.add(plain_sources)

    ksp_zipped_sources_output = None
    if ksp_annotation_processor_properties.annotation_processors:
        ksp_cmd = [compile_kotlin_tool]
        ksp_cmd.append(zip_scrubber_args)

        ksp_annotation_processor_classpath_tsets = filter(None, ([ap.deps for ap in ksp_annotation_processor_properties.annotation_processors]))
        if ksp_annotation_processor_classpath_tsets:
            ksp_annotation_processor_classpath = ctx.actions.tset(
                JavaPackagingDepTSet,
                children = ksp_annotation_processor_classpath_tsets,
            ).project_as_args("full_jar_args")
            ksp_cmd.append("--ksp_processor_jars")
            ksp_cmd.append(cmd_args(ksp_annotation_processor_classpath, delimiter = ","))

        ksp_cmd.extend(["--ksp_classpath", classpath_args])
        ksp_classes_and_resources_output = ctx.actions.declare_output(
            output_artifact_prefix, "ksp_output_dir/ksp_classes_and_resources_output", has_content_based_path = True
        )
        ksp_cmd.extend(["--ksp_classes_and_resources_output", ksp_classes_and_resources_output.as_output()])
        ksp_output = cmd_args(ksp_classes_and_resources_output.as_output(), parent = 1)
        ksp_cmd.extend(["--ksp_output", ksp_output])
        ksp_sources_output = ctx.actions.declare_output(output_artifact_prefix, "ksp_output_dir/ksp_sources_output", has_content_based_path = True)
        ksp_cmd.extend(["--ksp_sources_output", ksp_sources_output.as_output()])
        ksp_zipped_sources_output = ctx.actions.declare_output(
            output_artifact_prefix, "ksp_output_dir/ksp_zipped_sources_output.src.zip", has_content_based_path = True
        )
        ksp_cmd.extend(["--ksp_zipped_sources_output", ksp_zipped_sources_output.as_output()])
        ksp_cmd.extend(["--ksp_project_base_dir", ctx.label.path])

        ksp_kotlinc_cmd_args = cmd_args(kotlinc_cmd_args)
        plugins_cmd_args = _add_plugins(ctx, plugins, is_ksp = True)
        ksp_kotlinc_cmd_args.add(plugins_cmd_args.kotlinc_cmd_args)
        ksp_cmd.append(plugins_cmd_args.compile_kotlin_cmd)

        ksp_cmd_args_file, _ = ctx.actions.write(
            "{}ksp_kotlinc_cmd".format(output_artifact_prefix),
            ksp_kotlinc_cmd_args,
            allow_args = True,
            has_content_based_path = False,
        )

        ksp_cmd.extend(["--kotlinc_cmd_file", ksp_cmd_args_file])

        ctx.actions.run(
            cmd_args(ksp_cmd, hidden = ksp_kotlinc_cmd_args),
            category = "ksp_kotlinc",
            allow_cache_upload = True,
            error_handler = kotlin_toolchain.kotlin_error_handler,
        )

        zipped_sources = (zipped_sources or []) + [ksp_zipped_sources_output]
        compile_kotlin_cmd_args.extend(["--ksp_generated_classes_and_resources", ksp_classes_and_resources_output])

    plugin_cmd_args = _add_plugins(ctx, plugins, is_ksp = False)
    kotlinc_cmd_args.add(plugin_cmd_args.kotlinc_cmd_args)
    compile_kotlin_cmd_args.append(plugin_cmd_args.compile_kotlin_cmd)

    if zipped_sources:
        zipped_sources_file = ctx.actions.write("{}kotlinc_zipped_source_args".format(output_artifact_prefix), zipped_sources, has_content_based_path = False)
        compile_kotlin_cmd_args.append(["--zipped_sources_file", zipped_sources_file])
        compile_kotlin_cmd_hidden.add(zipped_sources)

    args_file, _ = ctx.actions.write(
        "{}kotlinc_cmd".format(output_artifact_prefix),
        kotlinc_cmd_args,
        allow_args = True,
        has_content_based_path = False,
    )

    compile_kotlin_cmd_hidden.add(plain_sources)

    compile_kotlin_cmd_args.append("--kotlinc_cmd_file")
    compile_kotlin_cmd_args.append(args_file)
    compile_kotlin_cmd_hidden.add(kotlinc_cmd_args)

    ctx.actions.run(
        cmd_args(compile_kotlin_cmd_args, hidden = compile_kotlin_cmd_hidden),
        category = "{}kotlinc".format(output_artifact_prefix),
        allow_cache_upload = True,
        error_handler = kotlin_toolchain.kotlin_error_handler,
    )

    return kotlinc_output, kapt_generated_sources_output, ksp_zipped_sources_output

def _is_ksp_plugin(plugin: str) -> bool:
    return "symbol-processing" in plugin

_PluginCmdArgs = record(
    kotlinc_cmd_args = cmd_args,
    compile_kotlin_cmd = cmd_args,
)

def _add_plugins(ctx: AnalysisContext, plugins: list[tuple], is_ksp: bool) -> _PluginCmdArgs:
    kotlinc_cmd_args = cmd_args()
    compile_kotlin_cmd = cmd_args()
    for plugin, plugin_options in plugins:
        if _is_ksp_plugin(str(plugin)) != is_ksp:
            continue

        kotlinc_cmd_args.add(cmd_args(["-Xplugin", plugin[DefaultInfo].default_outputs[0]], delimiter = "="))
        options = []
        for option_key, option_val in plugin_options.items():
            # "_codegen_dir_" means buck should provide a dir
            if option_val == "__codegen_dir__":
                option_val = ctx.actions.declare_output("kotlin_compiler_plugin_dir", has_content_based_path = True)
                options.append(cmd_args([option_key, option_val.as_output()], delimiter = "="))
                compile_kotlin_cmd.add(["--kotlin_compiler_plugin_dir", option_val.as_output()])
            else:
                options.append(cmd_args([option_key, option_val], delimiter = "="))

        if options:
            kotlinc_cmd_args.add(["-P", cmd_args(options, delimiter = ",")])

    return _PluginCmdArgs(kotlinc_cmd_args = kotlinc_cmd_args, compile_kotlin_cmd = compile_kotlin_cmd)

def get_language_version(ctx: AnalysisContext) -> str:
    kotlin_toolchain = ctx.attrs._kotlin_toolchain[KotlinToolchainInfo]

    # kotlin compiler expects relase version of format 1.6, 1.7, etc. Don't include patch version
    current_kotlin_release_version = ".".join(kotlin_toolchain.kotlin_version.split(".")[:2])

    current_language_version = None
    for arg in ctx.attrs.extra_kotlinc_arguments:
        # If `-language-version` is defined multiple times, we use the last one, just like the compiler does
        if "-language-version" in str(arg):
            current_language_version = str(arg).split("=")[1].strip(' "')

    if not current_language_version or (current_kotlin_release_version >= "2.0" and current_language_version < "2.0"):
        current_language_version = current_kotlin_release_version

    return current_language_version

def get_language_version_arg(ctx: AnalysisContext) -> list[str]:
    language_version = get_language_version(ctx)
    return ["-language-version=" + language_version]

def _kotlinc_arg_text(arg) -> str:
    """The text kotlincd gives kotlinc for one `extra_kotlinc_arguments`
    element: KotlinCStepsBuilder passes each element unchanged. `str()` of an
    `attrs.arg()` value is that text in one pair of double quotes, with a
    macro written as `$(location ...)`."""
    if isinstance(arg, str):
        return arg
    return str(arg)[1:-1]

def filter_out_language_version(extra_arguments: list) -> list:
    return [arg for arg in extra_arguments if not (isinstance(arg, str) and "-language-version" in arg)]

def kotlin_library_impl(ctx: AnalysisContext) -> list[Provider]:
    packaging_deps = ctx.attrs.deps + ctx.attrs.exported_deps + ctx.attrs.runtime_deps

    # TODO(T107163344) this shouldn't be in kotlin_library itself, use overlays to remove it.
    android_packageable_info = merge_android_packageable_info(ctx.label, ctx.actions, packaging_deps)
    if ctx.attrs._build_only_native_code:
        shared_library_info, cxx_resource_info, linkable_graph = create_native_providers(ctx, ctx.label, packaging_deps)
        return [
            shared_library_info,
            cxx_resource_info,
            linkable_graph,
            # Add an unused default output in case this target is used an an attr.source() anywhere.
            DefaultInfo(default_output = ctx.actions.write("{}/unused.jar".format(ctx.label.name), [], has_content_based_path = False)),
            TemplatePlaceholderInfo(
                keyed_variables = {
                    "classpath": "unused_but_needed_for_analysis",
                }
            ),
            android_packageable_info,
        ]

    target_stats_providers, target_stats_subtargets = jvm_target_stats(ctx)

    java_providers = build_kotlin_library(
        ctx = ctx,
        validation_deps_outputs = get_validation_deps_outputs(ctx),
        extra_sub_targets = target_stats_subtargets,
    )
    return to_list(java_providers) + [android_packageable_info] + target_stats_providers

def _check_exported_deps(exported_deps: list[Dependency], attr_name: str):
    for exported_dep in exported_deps:
        # TODO(navidq) add a check that the exported dep always have a JavaLibraryInfo provider
        if JavaLibraryInfo in exported_dep:
            expect(
                not exported_dep[JavaLibraryInfo].may_not_be_exported,
                "{} has 'may_not_be_exported' label and should not be present in {}.".format(exported_dep.label.raw_target(), attr_name),
            )

def build_kotlin_library(
    ctx: AnalysisContext,
    additional_classpath_entries: JavaCompilingDepsTSet | None = None,
    custom_jdk_info: CustomJdkInfo | None = None,
    extra_sub_targets: dict = {},
    validation_deps_outputs: [list[Artifact], None] = None,
    extra_arguments = [],
) -> JavaProviders:
    srcs = ctx.attrs.srcs
    has_kotlin_srcs = lazy.is_any(lambda src: src.extension == ".kt" or src.basename.endswith(".src.zip") or src.basename.endswith("-sources.jar"), srcs)
    kotlin_toolchain = ctx.attrs._kotlin_toolchain[KotlinToolchainInfo]
    abi_generation_mode = get_abi_generation_mode(ctx.attrs.abi_generation_mode)
    if abi_generation_mode == AbiGenerationMode("source_only") and kotlin_toolchain.kosabi_stubs_gen_k2_plugin == None:
        abi_generation_mode = AbiGenerationMode("class")

    if not has_kotlin_srcs:
        return build_java_library(
            ctx,
            ctx.attrs.srcs,
            custom_jdk_info = custom_jdk_info,
            additional_classpath_entries = additional_classpath_entries,
            override_abi_generation_mode = abi_generation_mode or AbiGenerationMode("class"),
            extra_sub_targets = extra_sub_targets,
            validation_deps_outputs = validation_deps_outputs,
            extra_arguments = extra_arguments,
        )

    else:
        deps_query = getattr(ctx.attrs, "deps_query", []) or []
        provided_deps_query = getattr(ctx.attrs, "provided_deps_query", []) or []
        _check_exported_deps(ctx.attrs.exported_deps, "exported_deps")
        _check_exported_deps(ctx.attrs.exported_provided_deps, "exported_provided_deps")
        deps = ctx.attrs.deps + deps_query + ctx.attrs.exported_deps + ctx.attrs.provided_deps + provided_deps_query + ctx.attrs.exported_provided_deps
        annotation_processor_properties = create_annotation_processor_properties(
            ctx,
            ctx.attrs.plugins + ctx.attrs.non_exec_dep_plugins_deprecated,
            ctx.attrs.annotation_processors,
            ctx.attrs.annotation_processor_params,
            ctx.attrs.annotation_processor_deps,
        )
        ksp_annotation_processor_properties = create_ksp_annotation_processor_properties(ctx.attrs.plugins + ctx.attrs.non_exec_dep_plugins_deprecated)

        # -bootclasspath is a javac-only, JDK<9 release concept, but we still use this field
        # to append additional jars to the kotlinc classpath. When we drop Java 8 language
        # level support someday, we should rework this field and treat it like additional
        # classpath entries.
        bootclasspath_for_kotlinc = custom_jdk_info.bootclasspath if custom_jdk_info else []
        bootclasspath_jar_snapshots_for_kotlinc = custom_jdk_info.bootclasspath_jar_snapshots if custom_jdk_info and ctx.attrs.incremental else []

        javac_tool = derive_javac(ctx.attrs.javac) if getattr(ctx.attrs, "javac", None) else None
        source_level, target_level = get_java_version_attributes(ctx)

        if javac_tool or kotlin_toolchain.kotlinc_protocol == "classic":
            kotlinc_classes, kapt_generated_sources, ksp_generated_sources = _create_kotlin_sources(
                ctx,
                ctx.attrs.srcs,
                (deps or []) + [kotlin_toolchain.kotlin_stdlib],
                annotation_processor_properties,
                ksp_annotation_processor_properties,
                additional_classpath_entries,
                bootclasspath_for_kotlinc,
                ctx.attrs.kotlin_compiler_plugins,
                target_level,
            )
            semanticdb_res = _semanticdb_plugin(ctx, kotlin_toolchain)
            if not ctx.attrs._is_building_android_binary and semanticdb_res:
                (semanticdb_plugin, semanticdb_output) = semanticdb_res
                _create_kotlin_sources(
                    ctx,
                    ctx.attrs.srcs,
                    (deps or []) + [kotlin_toolchain.kotlin_stdlib],
                    annotation_processor_properties,
                    ksp_annotation_processor_properties,
                    additional_classpath_entries,
                    bootclasspath_for_kotlinc,
                    semanticdb_plugin,
                    target_level,
                    output_artifact_prefix = "semanticdb",
                )
                extra_sub_targets = extra_sub_targets | {"semanticdb": [DefaultInfo(default_output = semanticdb_output)]}

            srcs = [src for src in ctx.attrs.srcs if not src.extension == ".kt"]
            if kapt_generated_sources:
                srcs.append(kapt_generated_sources)
            if ksp_generated_sources:
                srcs.append(ksp_generated_sources)
            all_additional_classpath_entries = get_compiling_deps_tset(
                ctx.actions,
                value = JavaClasspathEntry(
                    full_library = kotlinc_classes,
                    abi = kotlinc_classes,
                    abi_as_dir = None,
                    required_for_source_only_abi = True,
                    abi_jar_snapshot = None,
                ),
                additional_classpath_entries = ([additional_classpath_entries] if additional_classpath_entries else [])
                + [kotlin_toolchain.kotlin_stdlib[JavaLibraryInfo].compiling_deps],
            )
            java_lib = build_java_library(
                ctx,
                srcs,
                run_annotation_processors = False,
                custom_jdk_info = custom_jdk_info,
                additional_classpath_entries = all_additional_classpath_entries,
                additional_compiled_srcs = kotlinc_classes,
                generated_sources = filter(None, [kapt_generated_sources, ksp_generated_sources]),
                extra_sub_targets = extra_sub_targets,
                validation_deps_outputs = validation_deps_outputs,
                extra_arguments = extra_arguments,
            )
            return java_lib
        elif kotlin_toolchain.kotlinc_protocol == "kotlincd":
            expect(
                ctx.attrs._java_toolchain[JavaToolchainInfo].javac_protocol == "javacd",
                "Kotlin compiler mode: kotlincd and java compiler mode: {} don't match.".format(ctx.attrs._java_toolchain[JavaToolchainInfo].javac_protocol)
                + "\nHint: If you have a Java toolchain with a custom javac, you should also provide a custom kotlinc for your Kotlin toolchain.",
            )
            extra_arguments = cmd_args(
                ctx.attrs.extra_arguments + extra_arguments,
                # The outputs of validation_deps need to be added as hidden arguments
                # to an action for the validation_deps targets to be built and enforced.
                hidden = validation_deps_outputs or [],
            )

            kosabi_applicability_runs = abi_generation_mode == AbiGenerationMode("source_only") and kotlin_toolchain.kosabi_applicability_plugin != None
            # Applicability verdicts need whole-target analysis; incremental frontends can omit findings from unchanged files.
            incremental = ctx.attrs.incremental and not kosabi_applicability_runs

            common_kotlincd_kwargs = {
                "abi_generation_mode": abi_generation_mode,
                "actions": ctx.actions,
                "additional_classpath_entries": additional_classpath_entries,
                "annotation_processor_properties": AnnotationProcessorProperties(
                    annotation_processors = annotation_processor_properties.annotation_processors + ksp_annotation_processor_properties.annotation_processors,
                    annotation_processor_params = annotation_processor_properties.annotation_processor_params
                    + ksp_annotation_processor_properties.annotation_processor_params,
                    source_only_abi_annotation_processor_param_overrides = annotation_processor_properties.source_only_abi_annotation_processor_param_overrides,
                ),
                "bootclasspath_entries": bootclasspath_for_kotlinc,
                "custom_jdk_info": custom_jdk_info,
                "debug_port": getattr(ctx.attrs, "debug_port", None),
                "deps": deps + [kotlin_toolchain.kotlin_stdlib],
                "enable_depfiles": getattr(ctx.attrs, "enable_depfiles", True),
                "extra_kotlinc_arguments": filter_out_language_version(ctx.attrs.extra_kotlinc_arguments or []),
                "friend_paths": get_friend_paths(ctx),
                "is_building_android_binary": ctx.attrs._is_building_android_binary,
                "jar_postprocessor": ctx.attrs.jar_postprocessor[RunInfo] if hasattr(ctx.attrs, "jar_postprocessor") and ctx.attrs.jar_postprocessor else None,
                "java_toolchain": ctx.attrs._java_toolchain[JavaToolchainInfo],
                "kotlin_compiler_plugins": ctx.attrs.kotlin_compiler_plugins,
                "kotlin_toolchain": kotlin_toolchain,
                "label": ctx.label,
                "language_version": get_language_version(ctx),
                "manifest_file": ctx.attrs.manifest_file,
                "remove_classes": ctx.attrs.remove_classes,
                "required_for_source_only_abi": ctx.attrs.required_for_source_only_abi,
                "resources": ctx.attrs.resources,
                "resources_root": ctx.attrs.resources_root,
                "source_level": source_level,
                "source_only_abi_deps": ctx.attrs.source_only_abi_deps,
                "srcs": srcs,
                "target_level": target_level,
            }

            outputs, proto, tracking_outputs = create_jar_artifact_kotlincd(
                plugin_params = create_plugin_params(ctx, ctx.attrs.plugins + ctx.attrs.non_exec_dep_plugins_deprecated),
                extra_arguments = extra_arguments,
                actions_identifier = "",
                incremental = incremental,
                bootclasspath_snapshot_entries = bootclasspath_jar_snapshots_for_kotlinc if incremental else [],
                track_files_which_skipped_compilation = kotlin_toolchain.track_files_which_skipped_compilation,
                **common_kotlincd_kwargs,
            )

            if proto:
                extra_sub_targets = extra_sub_targets | {"jar_command_proto_json": [DefaultInfo(default_output = proto)]}
            if outputs and outputs.incremental_state_dir:
                extra_sub_targets = extra_sub_targets | {
                    "incremental_state_dir": [
                        DefaultInfo(default_output = outputs.incremental_state_dir),
                    ]
                }

            for subtarget_name, tracking_artifact in tracking_outputs.items():
                extra_sub_targets = extra_sub_targets | {
                    subtarget_name: [
                        DefaultInfo(default_output = tracking_artifact),
                    ]
                }

            if outputs and outputs.annotation_processor_output:
                generated_sources = [outputs.annotation_processor_output]
                extra_sub_targets = extra_sub_targets | {
                    "generated_sources": [
                        DefaultInfo(default_output = outputs.annotation_processor_output),
                    ]
                }
            else:
                generated_sources = []

            java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
            if not java_toolchain.is_bootstrap_toolchain and not ctx.attrs._is_building_android_binary:
                # Skip Nullsafe for Kotlin-only targets (no real .java) — its javac step crashes on them.
                if lazy.is_any(lambda src: src.extension == ".java", srcs):
                    extra_sub_targets = _nullsafe_subtarget(ctx, extra_sub_targets, common_kotlincd_kwargs)
                extra_sub_targets = _semanticdb_subtarget(ctx, extra_sub_targets, kotlin_toolchain, java_toolchain, common_kotlincd_kwargs)
                extra_sub_targets = _typecheck_subtarget(
                    ctx,
                    extra_sub_targets,
                    kotlin_toolchain,
                    srcs,
                    deps,
                    additional_classpath_entries,
                    bootclasspath_for_kotlinc,
                    generated_sources,
                    annotation_processor_properties,
                    ksp_annotation_processor_properties,
                    target_level,
                )

            class_to_src_map, sources_jar, class_to_src_map_sub_targets = get_class_to_source_map_info(
                ctx,
                outputs = outputs,
                deps = ctx.attrs.deps + deps_query + ctx.attrs.exported_deps,
                generate_sources_jar = True,
            )
            extra_sub_targets = extra_sub_targets | class_to_src_map_sub_targets

            (
                java_library_info,
                java_packaging_info,
                global_code_info,
                shared_library_info,
                cxx_resource_info,
                linkable_graph,
                template_placeholder_info,
                intellij_info,
            ) = create_java_library_providers(
                ctx,
                library_output = outputs.classpath_entry if outputs else None,
                global_code_config = java_toolchain.global_code_config,
                declared_deps = ctx.attrs.deps + deps_query,
                exported_deps = ctx.attrs.exported_deps,
                provided_deps = ctx.attrs.provided_deps + provided_deps_query,
                exported_provided_deps = ctx.attrs.exported_provided_deps,
                runtime_deps = ctx.attrs.runtime_deps,
                generated_sources = generated_sources,
                has_srcs = bool(srcs),
                sources_jar = sources_jar,
                preprocessed_library = outputs.preprocessed_library if outputs else None,
                used_jars_json = outputs.used_jars_json if outputs else None,
            )

            default_info = get_default_info(
                ctx.actions,
                ctx.attrs._java_toolchain[JavaToolchainInfo],
                outputs,
                java_packaging_info,
                extra_sub_targets = extra_sub_targets,
            )

            validation_specs = get_attrs_validation_specs(ctx)
            validation_info = ValidationInfo(validations = validation_specs) if validation_specs else None

            return JavaProviders(
                java_library_info = java_library_info,
                java_library_intellij_info = intellij_info,
                java_packaging_info = java_packaging_info,
                java_global_code_info = global_code_info,
                shared_library_info = shared_library_info,
                cxx_resource_info = cxx_resource_info,
                linkable_graph = linkable_graph,
                template_placeholder_info = template_placeholder_info,
                default_info = default_info,
                class_to_src_map = class_to_src_map,
                validation_info = validation_info,
            )
        else:
            fail("unrecognized kotlinc protocol `{}`".format(kotlin_toolchain.kotlinc_protocol))

def _nullsafe_subtarget(ctx: AnalysisContext, extra_sub_targets: dict, common_kotlincd_kwargs: dict):
    nullsafe_info = get_nullsafe_info(ctx)
    if nullsafe_info:
        create_jar_artifact_kotlincd(
            actions_identifier = "nullsafe",
            plugin_params = nullsafe_info.plugin_params,
            extra_arguments = nullsafe_info.extra_arguments,
            # To make sure that even for pure Kotlin targets empty output dir is always present
            optional_dirs = [nullsafe_info.output.as_output()],
            is_creating_subtarget = True,
            incremental = False,
            bootclasspath_snapshot_entries = [],
            **common_kotlincd_kwargs,
        )

        extra_sub_targets = extra_sub_targets | {
            "nullsafex-json": [
                DefaultInfo(default_output = nullsafe_info.output),
            ]
        }
    return extra_sub_targets

def _semanticdb_plugin(ctx: AnalysisContext, kotlin_toolchain: KotlinToolchainInfo) -> tuple | None:
    sourceroot = kotlin_toolchain.semanticdb_sourceroot
    if not sourceroot:
        return None
    kotlin_version = get_language_version(ctx)
    semanticdb_kotlinc_plugins = kotlin_toolchain.semanticdb_kotlinc
    semanticdb_kotlinc = semanticdb_kotlinc_plugins.get(kotlin_version) if semanticdb_kotlinc_plugins else None
    if not semanticdb_kotlinc:
        return None
    semanticdb_kotlinc_output = ctx.actions.declare_output("semanticdb", "out", dir = True, has_content_based_path = False)
    semanticdb_plugin = [
        (
            semanticdb_kotlinc,
            {
                "plugin:semanticdb-kotlinc:sourceroot": sourceroot,
                "plugin:semanticdb-kotlinc:targetroot": cmd_args(semanticdb_kotlinc_output.as_output()),
            },
        )
    ]
    return (semanticdb_plugin, semanticdb_kotlinc_output)

def _semanticdb_subtarget(
    ctx: AnalysisContext, extra_sub_targets: dict, kotlin_toolchain: KotlinToolchainInfo, java_toolchain: JavaToolchainInfo, common_kotlincd_kwargs: dict
):
    sourceroot = kotlin_toolchain.semanticdb_sourceroot
    if not sourceroot:
        return extra_sub_targets
    semanticdb_javac = java_toolchain.semanticdb_javac
    semanticdb_kotlinc_plugins = kotlin_toolchain.semanticdb_kotlinc
    kotlin_version = get_language_version(ctx)
    semanticdb_kotlinc = kotlin_toolchain.semanticdb_kotlinc.get(kotlin_version) if semanticdb_kotlinc_plugins else None
    if semanticdb_javac or semanticdb_kotlinc:
        semanticdb_output = ctx.actions.declare_output("semanticdb", dir = True, has_content_based_path = False)
        semanticdb_javac_plugin_params = None
        if semanticdb_javac:
            javac_sourceroot_args = cmd_args(sourceroot, format = "-sourceroot:{}")
            javac_targetroot_args = cmd_args(semanticdb_output.as_output(), format = "-targetroot:{}")
            semanticdb_javac_plugin_params = create_plugin_params(
                ctx,
                [(semanticdb_javac, cmd_args(javac_sourceroot_args, javac_targetroot_args))],
            )
        kwargs = common_kotlincd_kwargs
        if semanticdb_kotlinc:
            # IMPORTANT: Append semanticdb to existing plugins, don't replace them
            # This ensures other plugins (like Ultralight DI) continue to work
            existing_plugins = common_kotlincd_kwargs.get("kotlin_compiler_plugins", [])
            semanticdb_plugin = (
                semanticdb_kotlinc,
                {
                    "plugin:semanticdb-kotlinc:sourceroot": sourceroot,
                    "plugin:semanticdb-kotlinc:targetroot": cmd_args(semanticdb_output.as_output()),
                },
            )
            kotlin_compiler_plugins = existing_plugins + [semanticdb_plugin]
            kwargs = common_kotlincd_kwargs | {"kotlin_compiler_plugins": kotlin_compiler_plugins}
        create_jar_artifact_kotlincd(
            actions_identifier = "semanticdb",
            plugin_params = semanticdb_javac_plugin_params,
            extra_arguments = cmd_args(),
            optional_dirs = [],
            is_creating_subtarget = True,
            incremental = False,
            bootclasspath_snapshot_entries = [],
            **kwargs,
        )
        extra_sub_targets = extra_sub_targets | {
            "semanticdb": [
                DefaultInfo(default_output = semanticdb_output),
            ]
        }
    return extra_sub_targets

def _typecheck_subtarget(
    ctx: AnalysisContext,
    extra_sub_targets: dict,
    kotlin_toolchain: KotlinToolchainInfo,
    srcs: list[Artifact],
    deps: list[Dependency],
    additional_classpath_entries: JavaCompilingDepsTSet | None,
    bootclasspath_entries: list[Artifact],
    generated_sources: list[Artifact],
    annotation_processor_properties,
    ksp_annotation_processor_properties,
    target_level: int,
) -> dict:
    if not kotlin_toolchain.typechecker_enabled:
        return extra_sub_targets
    typechecker_cli = kotlin_toolchain.typechecker_cli
    typechecker_wrapper = kotlin_toolchain.typechecker_wrapper
    if not typechecker_cli or not typechecker_wrapper:
        return extra_sub_targets

    # Sources: plain .kt plus kotlinc-style source archives (the wrapper unzips;
    # suffixes mirror _ARCHIVE_SUFFIXES in typechecker.py).
    # Mixed .java sources ride along as declaration stubs (the checker
    # scans their top-level types for import resolution but never checks
    # them); java_sources_skipped stays 0 (report-schema compat).
    kt_srcs = [s for s in srcs if s.extension == ".kt"]
    zip_srcs = [s for s in srcs if s.basename.endswith(".src.zip") or s.basename.endswith("-sources.jar")]
    java_srcs = [s for s in srcs if s.extension == ".java"]

    # Compiling classpath (ABI jars) in the order kotlincd hands kotlinc
    # (ClasspathUtils.getAllClasspaths): the compiling deps, projected as in
    # cd_jar_creator_util.bzl, then the bootclasspath (kotlincd extraClassPaths).
    compiling_deps_tset = get_compiling_deps_tset(
        ctx.actions,
        deps + [kotlin_toolchain.kotlin_stdlib],
        [additional_classpath_entries] if additional_classpath_entries else [],
    )
    compiling_classpath = cmd_args()
    if compiling_deps_tset:
        compiling_classpath.add(compiling_deps_tset.project_as_args("args_for_compiling", ordering = "topological"))
    friend_jars = [friend_path.library_output.abi for friend_path in map_idx(JavaLibraryInfo, get_friend_paths(ctx)) if friend_path.library_output]
    compiling_classpath.add(friend_jars)
    compiling_classpath.add(bootclasspath_entries)

    # kotlincd passes neither -no-jdk nor -jdk-home, so kotlinc also compiles
    # against the JDK it runs on, for Android targets too, after every
    # classpath root (kotlin-2.3.21 ClasspathRootsResolver.computeRoots).
    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    compiling_classpath.add(filter(None, [java_toolchain.jdk_modules_jar]))
    jdk_base_jar = java_toolchain.java_base_jar
    classpath_args = cmd_args(compiling_classpath, delimiter = get_path_separator_for_exec_os(ctx))
    classpath_file, _ = ctx.actions.write("typecheck_classpath", classpath_args, allow_args = True, has_content_based_path = False)

    # D4 plugin detection: any compiler plugin, kapt/ksp processor, or
    # processor dep marks the target (WARN, never error; recorded below).
    has_processors = bool(
        annotation_processor_properties.annotation_processors
        or ksp_annotation_processor_properties.annotation_processors
        or ctx.attrs.annotation_processor_deps
    )
    has_plugins = bool(ctx.attrs.kotlin_compiler_plugins) or has_processors

    # Of what kotlincd generates, only annotation processor output is compiler
    # input; compiler plugins run inside kotlinc. Without processors there is
    # nothing generated to check, and an input on the generated-sources dir
    # would only make this action wait for kotlincd.
    checked_generated_sources = generated_sources if has_processors else []

    # kotlinc matches a flag on the exact text of its argument, so each
    # decision below reads an element's text as kotlinc receives it. A
    # substring test would false-match -Werror inside unrelated values, and
    # `"-Werror"` (quotes included) or ` -Werror` is a source path to kotlinc,
    # not -Werror.
    def has_flag(*names):
        return lazy.is_any(
            lambda a: _kotlinc_arg_text(a) in names,
            ctx.attrs.extra_kotlinc_arguments or [],
        )

    werror = has_flag("-Werror")
    nowarn = has_flag("-nowarn", "-W")

    # Per-warning severity overrides (kotlinc -Xwarning-level=NAME:LEVEL and
    # its deprecated alias -Xsuppress-warning=NAME) change which warnings
    # kotlinc reports, so the checker must see them too.
    warning_levels = []
    for a in ctx.attrs.extra_kotlinc_arguments or []:
        flag = _kotlinc_arg_text(a)
        if flag.startswith("-Xwarning-level="):
            warning_levels.append(flag[len("-Xwarning-level=") :])
        elif flag.startswith("-Xsuppress-warning="):
            warning_levels.append(flag[len("-Xsuppress-warning=") :] + ":disabled")

    # Every other kotlinc argument reaches the checker, in the order kotlincd
    # passes them (KotlinCStepsBuilder.getKotlincExtraArguments), so repeated
    # flags resolve the same: each compiler plugin's options, the JVM target,
    # -Xjspecify-annotations=strict (both compile paths add it), then
    # extra_kotlinc_arguments without the flags handled above, each with the
    # text it was tested on. A `__codegen_dir__` option value passes as the
    # literal: the checker runs no plugin and writes no plugin output.
    kotlinc_args = []
    for plugin, options in ctx.attrs.kotlin_compiler_plugins:
        if options and not _is_ksp_plugin(str(plugin)):
            kotlinc_args += ["-P", ",".join(["{}={}".format(key, value) for key, value in options.items()])]
    kotlinc_args += ["-jvm-target", get_kotlinc_compatible_target(str(target_level)), "-Xjspecify-annotations=strict"]
    for a in ctx.attrs.extra_kotlinc_arguments or []:
        flag = _kotlinc_arg_text(a)
        handled = flag in ("-Werror", "-nowarn", "-W") or "-language-version" in flag
        if not handled and not flag.startswith("-Xwarning-level=") and not flag.startswith("-Xsuppress-warning="):
            kotlinc_args.append(a)

    report = ctx.actions.declare_output("typecheck_report.json")
    args = cmd_args([
        typechecker_wrapper[RunInfo],
        "--typechecker-binary",
        typechecker_cli[RunInfo],
        "--classpath-file",
        classpath_file,
        "--srcs",
    ])
    args.add(kt_srcs + zip_srcs + java_srcs)
    args.add(["--output", report.as_output()])
    args.add(["--java-sources-skipped", "0"])
    for generated in checked_generated_sources:
        args.add(["--generated", generated])
    if werror:
        args.add("--werror")
    if nowarn:
        args.add("--nowarn")
    for level in warning_levels:
        args.add(["--warning-level", level])
    for a in kotlinc_args:
        args.add(cmd_args("--kotlinc-arg=", a, delimiter = ""))
    if has_plugins:
        args.add("--plugins")

    # The checker generates plugin declarations (e.g. kotlinx.serialization
    # `serializer()`) only for plugins the compilation registers.
    for plugin, _options in ctx.attrs.kotlin_compiler_plugins:
        args.add(["--kotlin-plugin", plugin[DefaultInfo].default_outputs[0]])

    # Friend jars ride the classpath like any dep; the checker must also know
    # they are friends (kotlinc -Xfriend-paths) to see their internals.
    for friend_jar in friend_jars:
        args.add(["--friend-path", friend_jar])
    args.add(["--language-version", get_language_version(ctx)])
    args.add("--jdk-required")
    if jdk_base_jar:
        args.add(["--jdk-base", jdk_base_jar])
    hidden = cmd_args([compiling_classpath, kt_srcs, zip_srcs, java_srcs, checked_generated_sources, classpath_file])
    ctx.actions.run(cmd_args(args, hidden = hidden), category = "typecheck")
    return extra_sub_targets | {
        "typecheck": [DefaultInfo(default_output = report)],
    }

kotlin_library = prelude_rule(
    name = "kotlin_library",
    impl = kotlin_library_impl,
    docs = """
        A `kotlin_library()` rule is used to define a set of Kotlin
        files that can be compiled together. The main output of a
         `kotlin_library()` rule is a single JAR file containing all
        of the compiled class files, as well as the static files specified in
        the `resources` argument.
    """,
    examples = """
        ```
        # A rule that compiles a single .kt file.
        kotlin_library(
          name = 'JsonUtil',
          srcs = ['JsonUtil.kt'],
          deps = [
            '//third_party/guava:guava',
            '//third_party/jackson:jackson',
          ],
        )

        # A rule that compiles all of the .kt files under the directory in
        # which the rule is defined using glob(). It also excludes an
        # individual file that may have additional dependencies, so it is
        # compiled by a separate rule.
        kotlin_library(
          name = 'messenger',
          srcs = glob(['**/*.kt'], excludes = ['MessengerModule.kt']),
          deps = [
            '//src/com/facebook/base:base',
            '//third_party/guava:guava',
          ],
        )

        kotlin_library(
          name = 'MessengerModule',
          srcs = ['MessengerModule.kt'],
          deps = [
            '//src/com/facebook/base:base',
            '//src/com/google/inject:inject',
            '//third_party/guava:guava',
            '//third_party/jsr-330:jsr-330',
          ],
        )

        # A rule that builds a library with both relative and
        # fully-qualified deps.
        kotlin_library(
          name = 'testutil',
          srcs = glob(['tests/**/*.kt'], excludes = 'tests/**/*Test.kt'),
          deps = [
            ':lib-fb4a',
            '//java/com/facebook/base:base',
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
                The set of `.kt`, `.java` or `.kts` files to compile for this rule.
                 If any of the files in this list end in `.src.zip`,
                 then the entries in the ZIP file that end in `.java` or `.kt` will be
                 included as ordinary inputs to compilation.
            """,
            ),
        }
        | jvm_common.resources_arg()
        | {
            "deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                Rules (usually other `kotlin_library` rules) that are used to
                 generate the classpath required to compile this `kotlin_library`.
            """,
            ),
            "extra_kotlinc_arguments": attrs.list(
                attrs.arg(anon_target_compatible = True),
                default = [],
                doc = """
                List of additional arguments to pass into the Kotlin compiler.
            """,
            ),
            "friend_paths": jvm_common.friend_paths_attr(),
        }
        | jvm_common.annotation_processors()
        | jvm_common.remove_classes_arg()
        | jvm_common.exported_deps()
        | jvm_common.provided_deps()
        | jvm_common.exported_provided_deps()
        | jvm_common.kotlin_compiler_plugins()
        | jvm_common.incremental()
        | jvm_common.plugins()
        | jvm_common.javac()
        | jvm_common.classic_java_content_based_paths()
        | buck.labels_arg()
        | jvm_common.abi_generation_mode()
        | {
            "extra_arguments": attrs.list(attrs.string(), default = []),
            "jar_postprocessor": attrs.option(attrs.exec_dep(), default = None),
            "java_version": attrs.option(attrs.string(), default = None),
            "jdk_release": attrs.option(attrs.string(), default = None),
            "keep_synthetics_in_class_abi": attrs.option(attrs.bool(), default = None),
            "manifest_file": attrs.option(attrs.source(), default = None),
            "maven_coords": attrs.option(attrs.string(), default = None),
            "min_sdk_version": attrs.default_only(attrs.option(attrs.int(), default = min_sdk_version())),
            "proguard_config": attrs.option(attrs.source(), default = None),
            "required_for_source_only_abi": attrs.bool(default = False),
            "resources_root": attrs.option(attrs.string(), default = None),
            "runtime_deps": attrs.list(attrs.dep(), default = []),
            "source": attrs.option(attrs.string(), default = None),
            "source_only_abi_deps": attrs.list(attrs.dep(), default = []),
            "target": attrs.option(attrs.string(), default = None),
            VALIDATION_DEPS_ATTR_NAME: attrs.set(attrs.dep(), sorted = True, default = []),
            "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
            "_dex_toolchain": toolchains_common.dex(),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_is_building_android_binary": is_building_android_binary_attr(),
            "_java_toolchain": toolchains_common.java(),
            "_kotlin_toolchain": toolchains_common.kotlin(),
        }
        | no_x_jdk_release_arg()
        | buck.licenses_arg()
        | buck.contacts_arg()
        | jvm_common.plugins()
        | validation_common.attrs_validators_arg()
    ),
)
