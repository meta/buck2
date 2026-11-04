# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:attrs_validators.bzl", "get_attrs_validation_specs", "validation_common")
load("@prelude//:paths.bzl", "paths")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//:validation_deps.bzl", "VALIDATION_DEPS_ATTR_NAME", "get_validation_deps_outputs")
load("@prelude//android:android_providers.bzl", "merge_android_packageable_info")
load("@prelude//android:build_only_native_code.bzl", "is_build_only_native_code")
load("@prelude//android:configuration.bzl", "is_building_android_binary_attr")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//graphql:graphql.bzl", "graphql_providers")
load("@prelude//java:compiler_depth.bzl", "compiler_depth_for_action")
load(
    "@prelude//java:java_providers.bzl",
    "ClasspathSnapshotGranularity",
    "JavaCompileOutputs",  # @unused Used as type
    "JavaCompilingDepsTSet",  # @unused Used as type
    "JavaLibraryInfo",
    "JavaPackagingDepTSet",
    "JavaProviders",
    "create_abi",
    "create_java_library_providers",
    "create_native_providers",
    "generate_java_classpath_snapshot",
    "get_compiling_deps_tset",
    "make_compile_outputs",
    "to_list",
)
load("@prelude//java:java_resources.bzl", "get_resources_map")
load("@prelude//java:java_toolchain.bzl", "AbiGenerationMode", "JavaToolchainInfo")
load("@prelude//java:javacd_jar_creator.bzl", "create_jar_artifact_javacd")
load("@prelude//java/plugins:java_annotation_processor.bzl", "AnnotationProcessorProperties", "create_annotation_processor_properties")
load(
    "@prelude//java/plugins:java_plugin.bzl",
    "PluginParams",  # @unused Used as type
    "create_plugin_params",
)
load("@prelude//java/utils:java_more_utils.bzl", "get_path_separator_for_exec_os")
load(
    "@prelude//java/utils:java_utils.bzl",
    "CustomJdkInfo",  # @unused Used as a type,
    "build_bootclasspath",
    "declare_prefixed_name",
    "derive_javac",
    "get_abi_generation_mode",
    "get_class_to_source_map_info",
    "get_default_info",
    "get_java_version_attributes",
    "to_java_version",
)
load("@prelude//jvm:attrs.bzl", "jvm_common", "min_sdk_version")
load("@prelude//jvm:cd_jar_creator_util.bzl", "postprocess_jar")
load("@prelude//jvm:nullsafe.bzl", "get_nullsafe_info")
load("@prelude//linking:shared_libraries.bzl", "SharedLibraryInfo")
load("@prelude//target_stats:target_stats.bzl", "CycleMode", "target_stats_providers_and_subtargets")
load("@prelude//target_stats:target_stats_config.bzl", "TARGET_STATS_ENABLED")
load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:label_provider.bzl", "LabelInfo")

_JAVA_FILE_EXTENSION = [".java"]
_SUPPORTED_ARCHIVE_SUFFIXES = [".src.zip", "-sources.jar"]

_JAVAC_PLUGIN_JVM_ARGS = [
    "-J--add-exports=jdk.compiler/com.sun.tools.javac.code=ALL-UNNAMED",
    "-J--add-exports=jdk.compiler/com.sun.tools.javac.comp=ALL-UNNAMED",
    "-J--add-exports=jdk.compiler/com.sun.tools.javac.file=ALL-UNNAMED",
    "-J--add-exports=jdk.compiler/com.sun.tools.javac.jvm=ALL-UNNAMED",
    "-J--add-exports=jdk.compiler/com.sun.tools.javac.main=ALL-UNNAMED",
    "-J--add-exports=jdk.compiler/com.sun.tools.javac.model=ALL-UNNAMED",
    "-J--add-exports=jdk.compiler/com.sun.tools.javac.processing=ALL-UNNAMED",
    "-J--add-exports=jdk.compiler/com.sun.tools.javac.parser=ALL-UNNAMED",
    "-J--add-exports=jdk.compiler/com.sun.tools.javac.tree=ALL-UNNAMED",
    "-J--add-opens=jdk.compiler/com.sun.tools.javac.api=ALL-UNNAMED",
    "-J--add-opens=jdk.compiler/com.sun.tools.javac.code=ALL-UNNAMED",
    "-J--add-opens=jdk.compiler/com.sun.tools.javac.jvm=ALL-UNNAMED",
    "-J--add-opens=jdk.compiler/com.sun.tools.javac.processing=ALL-UNNAMED",
    "-J--add-opens=jdk.compiler/com.sun.tools.javac.util=ALL-UNNAMED",
]

def _process_classpath(actions: AnalysisActions, classpath_args: cmd_args, args_file_name: str, option_name: str, has_content_based_path: bool) -> cmd_args:
    # write joined classpath string into args file
    classpath_args_file, _ = actions.write(
        args_file_name,
        classpath_args,
        allow_args = True,
        has_content_based_path = has_content_based_path,
    )

    return cmd_args(
        option_name,
        # add classpath args file to cmd
        classpath_args_file,
        # mark classpath artifacts as input
        hidden = classpath_args,
    )

def _classpath_args(ctx: AnalysisContext, args):
    return cmd_args(args, delimiter = get_path_separator_for_exec_os(ctx))

def _process_plugins(
    ctx: AnalysisContext,
    actions_identifier: [str, None],
    annotation_processor_properties: AnnotationProcessorProperties,
    plugin_params: [PluginParams, None],
    javac_args: cmd_args,
    use_content_based_paths: bool,
) -> cmd_args:
    cmd = cmd_args()
    processors_classpath_tsets = []

    # Process Annotation processors
    if annotation_processor_properties.annotation_processors:
        # For external javac, we can't preserve separate classpaths for separate processors. So we just concat everything.
        javac_args.add("-processor")
        joined_processors_string = ",".join([p for ap in annotation_processor_properties.annotation_processors for p in ap.processors])

        javac_args.add(joined_processors_string)
        for param in annotation_processor_properties.annotation_processor_params:
            javac_args.add("-A{}".format(param))

        for ap in annotation_processor_properties.annotation_processors:
            if ap.deps:
                processors_classpath_tsets.append(ap.deps)

    else:
        javac_args.add("-proc:none")

    # Process Javac Plugins
    if plugin_params:
        plugin, args = plugin_params.processors[0]

        # Produces "-Xplugin:PluginName arg1 arg2 arg3", as a single argument
        plugin_and_args = cmd_args(plugin)
        plugin_and_args.add(args)
        plugin_arg = cmd_args(format = "-Xplugin:{}", quote = "shell")
        plugin_arg.add(cmd_args(plugin_and_args, delimiter = " "))

        javac_args.add(plugin_arg)
        if plugin_params.deps:
            processors_classpath_tsets.append(plugin_params.deps)

    if len(processors_classpath_tsets) > 1:
        processors_classpath_tset = ctx.actions.tset(JavaPackagingDepTSet, children = processors_classpath_tsets)
    elif len(processors_classpath_tsets) == 1:
        processors_classpath_tset = processors_classpath_tsets[0]
    else:
        processors_classpath_tset = None

    if processors_classpath_tset:
        processors_classpath = _classpath_args(ctx, processors_classpath_tset.project_as_args("full_jar_args"))
        cmd.add(
            _process_classpath(
                ctx.actions,
                processors_classpath,
                declare_prefixed_name("plugin_cp_args", actions_identifier),
                "--javac_processors_classpath_file",
                use_content_based_paths,
            )
        )

    return cmd

def _build_classpath(
    actions: AnalysisActions,
    deps: list[Dependency],
    additional_classpath_entries: JavaCompilingDepsTSet | None,
    classpath_args_projection: str,
    additional_classpath_entries_list: list[Artifact],
) -> [cmd_args, None]:
    compiling_deps_tset = get_compiling_deps_tset(actions, deps)

    if additional_classpath_entries or compiling_deps_tset or additional_classpath_entries_list:
        args = cmd_args()
        if compiling_deps_tset:
            args.add(compiling_deps_tset.project_as_args(classpath_args_projection))
        if additional_classpath_entries:
            args.add(additional_classpath_entries.project_as_args(classpath_args_projection))
        if additional_classpath_entries_list:
            args.add(additional_classpath_entries_list)
        return args

    return None

def _append_javac_params(
    ctx: AnalysisContext,
    actions_identifier: [str, None],
    java_toolchain: JavaToolchainInfo,
    srcs: list[Artifact],
    remove_classes: list[str],
    annotation_processor_properties: AnnotationProcessorProperties,
    javac_plugin_params: [PluginParams, None],
    source_level: int,
    target_level: int,
    deps: list[Dependency],
    extra_arguments: cmd_args,
    additional_classpath_entries: JavaCompilingDepsTSet | None,
    custom_jdk_info: CustomJdkInfo | None,
    generated_sources_dir: Artifact,
    use_content_based_paths: bool,
) -> cmd_args:
    cmd = cmd_args()
    shared_javac_args = cmd_args(
        "-encoding",
        "utf-8",
        # Set the sourcepath to stop us reading source files out of jars by mistake.
        "-sourcepath",
        '""',
    )

    min_release_version = ctx.attrs.min_release_version if hasattr(ctx.attrs, "min_release_version") else None
    multi_release_srcs = ctx.attrs.multi_release_srcs if hasattr(ctx.attrs, "multi_release_srcs") else {}

    shared_javac_args.add(extra_arguments)

    additional_classpath_entries_list = []

    # min_release_version will use latest JDK for compilation, so we need to add JDK8 bootclasspath
    if target_level >= 9 or min_release_version:
        if custom_jdk_info:
            additional_classpath_entries_list = custom_jdk_info.bootclasspath
            shared_javac_args.add("--system", custom_jdk_info.system_image)
    else:
        custom_bootclasspath = custom_jdk_info.bootclasspath if custom_jdk_info else []
        bootclasspath_list = build_bootclasspath(custom_bootclasspath, source_level, java_toolchain)
        if bootclasspath_list:
            cmd.add(
                _process_classpath(
                    ctx.actions,
                    _classpath_args(ctx, bootclasspath_list),
                    declare_prefixed_name("bootclasspath_args", actions_identifier),
                    "--javac_bootclasspath_file",
                    use_content_based_paths,
                )
            )

    compiling_classpath = _build_classpath(ctx.actions, deps, additional_classpath_entries, "args_for_compiling", additional_classpath_entries_list)
    if compiling_classpath:
        cmd.add(
            _process_classpath(
                ctx.actions,
                _classpath_args(ctx, compiling_classpath),
                declare_prefixed_name("classpath_args", actions_identifier),
                "--javac_classpath_file",
                use_content_based_paths,
            )
        )
    else:
        shared_javac_args.add("-classpath ''")

    cmd.add(
        _process_plugins(
            ctx,
            actions_identifier,
            annotation_processor_properties,
            javac_plugin_params,
            shared_javac_args,
            use_content_based_paths,
        )
    )

    cmd.add("--generated_sources_dir", generated_sources_dir.as_output())

    zipped_sources, plain_sources = split_on_archives_and_plain_files(srcs, _JAVA_FILE_EXTENSION)

    # copy of javac_args to be used by multi_release_args_file before release is added and before srcs is added
    javac_args = shared_javac_args.copy()
    if min_release_version:
        javac_args.add("--release", min_release_version)
    else:
        javac_args.add("-source", str(source_level))
        javac_args.add("-target", str(target_level))
    javac_args.add(*plain_sources)
    args_file, _ = ctx.actions.write(
        declare_prefixed_name("javac_args", actions_identifier),
        javac_args,
        allow_args = True,
        has_content_based_path = use_content_based_paths,
    )
    cmd.add(cmd_args(hidden = javac_args))

    # mark plain srcs artifacts as input
    cmd.add(cmd_args(hidden = plain_sources))

    cmd.add("--javac_args_file", args_file)

    if min_release_version and multi_release_srcs:
        for min_release_version, release_srcs in multi_release_srcs.items():
            release_args = shared_javac_args.copy()
            release_args.add("--release", min_release_version)
            release_args.add(*release_srcs)
            release_args_file, _ = ctx.actions.write(
                declare_prefixed_name("multi_release_args.java{}".format(min_release_version), actions_identifier),
                release_args,
                allow_args = True,
                has_content_based_path = use_content_based_paths,
            )
            cmd.add("--multi_release_args_file", release_args_file)
            cmd.add(cmd_args(hidden = release_srcs))

    if zipped_sources:
        cmd.add(
            "--zipped_sources_file",
            ctx.actions.write(declare_prefixed_name("zipped_source_args", actions_identifier), zipped_sources, has_content_based_path = use_content_based_paths),
        )
        cmd.add(cmd_args(hidden = zipped_sources))

    if remove_classes:
        cmd.add(
            "--remove_classes",
            ctx.actions.write(declare_prefixed_name("remove_classes_args", actions_identifier), remove_classes, has_content_based_path = use_content_based_paths),
        )

    return cmd

def split_on_archives_and_plain_files(srcs: list[Artifact], plain_file_extensions: list[str]) -> (list[Artifact], list[Artifact]):
    archives = []
    plain_sources = []

    for src in srcs:
        if src.extension in plain_file_extensions:
            plain_sources.append(src)
        elif _is_supported_archive(src):
            archives.append(src)
        else:
            fail("Provided java source is not supported: {}".format(src))

    return (archives, plain_sources)

def _is_supported_archive(src: Artifact) -> bool:
    basename = src.basename
    for supported_suffix in _SUPPORTED_ARCHIVE_SUFFIXES:
        if basename.endswith(supported_suffix):
            return True
    return False

def _copy_resources(
    actions: AnalysisActions,
    actions_identifier: [str, None],
    java_toolchain: JavaToolchainInfo,
    package: str,
    resources: list[Artifact],
    resources_root: [str, None],
    use_content_based_paths: bool,
) -> Artifact:
    resources_to_copy = get_resources_map(java_toolchain, package, resources, resources_root)
    resource_output = actions.symlinked_dir(
        declare_prefixed_name("resources", actions_identifier), resources_to_copy, has_content_based_path = use_content_based_paths
    )
    return resource_output

def _jar_creator(javac_tool: [typing.Any, None], java_toolchain: JavaToolchainInfo) -> typing.Callable:
    if javac_tool or java_toolchain.javac_protocol == "classic":
        return _create_jar_artifact
    elif java_toolchain.javac_protocol == "javacd":
        return create_jar_artifact_javacd
    else:
        fail("unrecognized javac protocol `{}`".format(java_toolchain.javac_protocol))

def compile_to_jar(
    ctx: AnalysisContext,
    srcs: list[Artifact],
    *,
    abi_generation_mode: [AbiGenerationMode, None] = None,
    output: Artifact | None = None,
    actions_identifier: [str, None] = None,
    javac_tool: [typing.Any, None] = None,
    resources: [list[Artifact], None] = None,
    resources_root: [str, None] = None,
    remove_classes: [list[str], None] = None,
    manifest_file: Artifact | None = None,
    annotation_processor_properties: [AnnotationProcessorProperties, None] = None,
    plugin_params: [PluginParams, None] = None,
    source_level: [int, None] = None,
    target_level: [int, None] = None,
    deps: [list[Dependency], None] = None,
    required_for_source_only_abi: bool = False,
    source_only_abi_deps: [list[Dependency], None] = None,
    extra_arguments: [cmd_args, None] = None,
    additional_classpath_entries: JavaCompilingDepsTSet | None = None,
    additional_compiled_srcs: Artifact | None = None,
    custom_jdk_info: CustomJdkInfo | None = None,
    is_creating_subtarget: bool = False,
    debug_port: [int, None] = None,
    enable_depfiles: [bool, None] = True,
) -> JavaCompileOutputs:
    if not extra_arguments:
        extra_arguments = cmd_args()
    if not resources:
        resources = []
    if not deps:
        deps = []
    if not remove_classes:
        remove_classes = []
    if not annotation_processor_properties:
        annotation_processor_properties = AnnotationProcessorProperties(annotation_processors = [], annotation_processor_params = [])
    if not source_only_abi_deps:
        source_only_abi_deps = []

    # TODO(cjhopman): Should verify that source_only_abi_deps are contained within the normal classpath.

    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    if not source_level:
        source_level = to_java_version(java_toolchain.source_level)
    if not target_level:
        target_level = to_java_version(java_toolchain.target_level)

    is_building_android_binary = ctx.attrs._is_building_android_binary

    return _jar_creator(javac_tool, java_toolchain)(
        ctx,
        actions_identifier,
        abi_generation_mode,
        java_toolchain,
        ctx.label,
        output,
        javac_tool,
        srcs,
        remove_classes,
        resources,
        resources_root,
        manifest_file,
        annotation_processor_properties,
        plugin_params,
        source_level,
        target_level,
        deps,
        required_for_source_only_abi,
        source_only_abi_deps,
        extra_arguments,
        additional_classpath_entries,
        additional_compiled_srcs,
        custom_jdk_info,
        is_building_android_binary,
        is_creating_subtarget,
        debug_port,
        enable_depfiles,
    )

def _create_jar_artifact(
    ctx: AnalysisContext,
    actions_identifier: [str, None],
    abi_generation_mode: [AbiGenerationMode, None],
    java_toolchain: JavaToolchainInfo,
    label: Label,
    output: Artifact | None,
    javac_tool: [typing.Any, None],
    srcs: list[Artifact],
    remove_classes: list[str],
    resources: list[Artifact],
    resources_root: [str, None],
    manifest_file: Artifact | None,
    annotation_processor_properties: AnnotationProcessorProperties,
    plugin_params: [PluginParams, None],
    source_level: int,
    target_level: int,
    deps: list[Dependency],
    required_for_source_only_abi: bool,
    _source_only_abi_deps: list[Dependency],
    extra_arguments: cmd_args,
    additional_classpath_entries: JavaCompilingDepsTSet | None,
    additional_compiled_srcs: Artifact | None,
    custom_jdk_info: CustomJdkInfo | None,
    _is_building_android_binary: bool,
    _is_creating_subtarget: bool = False,
    _debug_port: [int, None] = None,
    _enable_depfiles: [bool, None] = True,
) -> JavaCompileOutputs:
    """
    Creates jar artifact.

    Returns a single artifacts that represents jar output file
    """
    use_content_based_paths = ctx.attrs.uses_content_based_paths_for_classic_java
    javac_tool = javac_tool or derive_javac(java_toolchain.javac)
    jar_out = output or ctx.actions.declare_output(
        paths.join(actions_identifier or "jar", "{}.jar".format(label.name)), has_content_based_path = use_content_based_paths
    )

    # since create_jar_artifact_javacd does not support this, it will not be added to common_compile_kwargs
    concat_resources = getattr(ctx.attrs, "concat_resources", False)

    args = [
        java_toolchain.compile_and_package[RunInfo],
        "--jar_builder_tool",
        cmd_args(java_toolchain.jar_builder, delimiter = " "),
        "--output",
        jar_out.as_output(),
    ]

    skip_javac = False if srcs or annotation_processor_properties.annotation_processors or plugin_params else True
    if skip_javac:
        args.append("--skip_javac_run")
    else:
        args += ["--javac_tool", javac_tool]
        if plugin_params:
            jvm_args_file = ctx.actions.write(
                declare_prefixed_name("javac_jvm_args", actions_identifier),
                _JAVAC_PLUGIN_JVM_ARGS,
                has_content_based_path = use_content_based_paths,
            )
            args += ["--javac_jvm_args_file", jvm_args_file]

    if resources:
        resource_dir = _copy_resources(ctx.actions, actions_identifier, java_toolchain, label.package, resources, resources_root, use_content_based_paths)
        args += ["--resources_dir", resource_dir]
    if concat_resources:
        args += ["--concat_resources"]

    if manifest_file:
        args += ["--manifest", manifest_file]

    if additional_compiled_srcs:
        args += ["--additional_compiled_srcs", additional_compiled_srcs]

    compile_and_package_cmd = cmd_args(args)

    generated_sources_dir = None
    if not skip_javac:
        generated_sources_dir = ctx.actions.declare_output(
            declare_prefixed_name("generated_sources", actions_identifier), dir = True, has_content_based_path = use_content_based_paths
        )
        compile_and_package_cmd.add(
            _append_javac_params(
                ctx,
                actions_identifier,
                java_toolchain,
                srcs,
                remove_classes,
                annotation_processor_properties,
                plugin_params,
                source_level,
                target_level,
                deps,
                extra_arguments,
                additional_classpath_entries,
                custom_jdk_info,
                generated_sources_dir,
                use_content_based_paths,
            )
        )

    ctx.actions.run(compile_and_package_cmd, category = "javac_and_jar", identifier = actions_identifier)

    abi = (
        None
        if (not srcs and not additional_compiled_srcs) or abi_generation_mode == AbiGenerationMode("none") or java_toolchain.is_bootstrap_toolchain
        else create_abi(ctx.actions, java_toolchain.class_abi_generator, jar_out, getattr(ctx.attrs, "keep_synthetics_in_class_abi", False) or False)
    )

    has_postprocessor = hasattr(ctx.attrs, "jar_postprocessor") and ctx.attrs.jar_postprocessor
    final_jar = (
        postprocess_jar(
            ctx.actions,
            java_toolchain.zip_scrubber,
            ctx.attrs.jar_postprocessor[RunInfo],
            java_toolchain.postprocessor_runner[RunInfo],
            jar_out,
            actions_identifier,
        )
        if has_postprocessor
        else jar_out
    )

    jar_snapshot = generate_java_classpath_snapshot(
        ctx.actions, java_toolchain.cp_snapshot_generator, ClasspathSnapshotGranularity("CLASS_MEMBER_LEVEL"), abi or final_jar, actions_identifier
    )
    # Classic javac is not counted, but preserve compiler depth through its inputs.
    compiler_depth = compiler_depth_for_action(
        label,
        "javac_and_jar",
        get_compiling_deps_tset(ctx.actions, deps, [additional_classpath_entries] if additional_classpath_entries else []) if not skip_javac else None,
    )
    return make_compile_outputs(
        full_library = final_jar,
        preprocessed_library = jar_out,
        class_abi = abi,
        required_for_source_only_abi = required_for_source_only_abi,
        annotation_processor_output = generated_sources_dir,
        abi_jar_snapshot = jar_snapshot,
        full_library_compiler_depth = compiler_depth,
        abi_compiler_depth = compiler_depth,
    )

def _check_dep_types(deps: list[Dependency]):
    for dep in deps:
        if JavaLibraryInfo not in dep and SharedLibraryInfo not in dep:
            fail("Received dependency {} is not supported. `java_library`, `prebuilt_jar` and native libraries are supported.".format(dep))

def _check_provided_deps(provided_deps: list[Dependency], attr_name: str):
    for provided_dep in provided_deps:
        expect(
            JavaLibraryInfo in provided_dep or SharedLibraryInfo not in provided_dep,
            "Java code does not need native libs in order to compile, so not valid as {}: {}".format(attr_name, provided_dep),
        )

def _check_exported_deps(exported_deps: list[Dependency], attr_name: str):
    for exported_dep in exported_deps:
        expect(
            JavaLibraryInfo in exported_dep,
            "Exported deps are meant to be forwarded onto the classpath for dependents, so only "
            + "make sense for a target that emits Java bytecode, {} in {} does not.".format(exported_dep, attr_name),
        )
        expect(
            not exported_dep[JavaLibraryInfo].may_not_be_exported,
            "{} has 'may_not_be_exported' label and should not be present in {}.".format(exported_dep.label.raw_target(), attr_name),
        )

# TODO(T145137403) remove need for this
def _skip_java_library_dep_checks(ctx: AnalysisContext) -> bool:
    return "skip_buck2_java_library_dep_checks" in ctx.attrs.labels

def jvm_target_stats(ctx: AnalysisContext) -> (list[Provider], dict[str, list[Provider]]):
    """target_stats for a JVM library rule (java_library, kotlin_library).

    JVM sources get package cycles, as android_library does; the tools come from
    the java toolchain, which is the one both rules have. A no-op unless the
    config is enabled and that toolchain carries the tools.
    """
    if not TARGET_STATS_ENABLED:
        return [], {}
    tools = ctx.attrs._java_toolchain[JavaToolchainInfo].target_stats_tools
    if tools == None:
        return [], {}
    return target_stats_providers_and_subtargets(
        ctx,
        tools = tools,
        srcs = {src.short_path: src for src in ctx.attrs.srcs},
        # Target stats cover the compile graph, including non-packaged provided deps.
        deps = (ctx.attrs.deps + ctx.attrs.exported_deps + ctx.attrs.runtime_deps + ctx.attrs.provided_deps + ctx.attrs.exported_provided_deps),
        cycle_mode = CycleMode("package"),
        module_name = ctx.label.name,
    )

def java_library_impl(ctx: AnalysisContext) -> list[Provider]:
    """
     java_library() rule implementation

    Args:
        ctx: rule analysis context
    Returns:
        list of created providers
    """
    packaging_deps = ctx.attrs.deps + ctx.attrs.exported_deps + ctx.attrs.runtime_deps

    # TODO(T107163344) this shouldn't be in java_library itself, use overlays to remove it.
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

    if not _skip_java_library_dep_checks(ctx):
        _check_dep_types(ctx.attrs.deps)
        _check_dep_types(ctx.attrs.provided_deps)
        _check_dep_types(ctx.attrs.exported_deps)
        _check_dep_types(ctx.attrs.exported_provided_deps)
        _check_dep_types(ctx.attrs.runtime_deps)

    target_stats_providers, target_stats_subtargets = jvm_target_stats(ctx)

    java_providers = build_java_library(
        ctx = ctx,
        srcs = ctx.attrs.srcs,
        validation_deps_outputs = get_validation_deps_outputs(ctx),
        extra_sub_targets = target_stats_subtargets,
    )

    return to_list(java_providers) + [android_packageable_info] + [LabelInfo(labels = ctx.attrs.labels)] + graphql_providers(ctx) + target_stats_providers

def build_java_library(
    ctx: AnalysisContext,
    srcs: list[Artifact],
    run_annotation_processors = True,
    additional_classpath_entries: JavaCompilingDepsTSet | None = None,
    custom_jdk_info: CustomJdkInfo | None = None,
    additional_compiled_srcs: Artifact | None = None,
    generated_sources: list[Artifact] = [],
    override_abi_generation_mode: [AbiGenerationMode, None] = None,
    extra_sub_targets: dict = {},
    validation_deps_outputs: [list[Artifact], None] = None,
    extra_arguments = [],
) -> JavaProviders:
    expect(
        not getattr(ctx.attrs, "_build_only_native_code", False),
        "Shouldn't call build_java_library if we're only building native code!",
    )

    _check_provided_deps(ctx.attrs.provided_deps, "provided_deps")
    _check_provided_deps(ctx.attrs.exported_provided_deps, "exported_provided_deps")
    _check_exported_deps(ctx.attrs.exported_deps, "exported_deps")
    _check_exported_deps(ctx.attrs.exported_provided_deps, "exported_provided_deps")

    deps_query = getattr(ctx.attrs, "deps_query", []) or []
    provided_deps_query = getattr(ctx.attrs, "provided_deps_query", []) or []
    first_order_deps = ctx.attrs.deps + deps_query + ctx.attrs.exported_deps + ctx.attrs.provided_deps + provided_deps_query + ctx.attrs.exported_provided_deps

    resources = ctx.attrs.resources
    resources_root = ctx.attrs.resources_root
    expect(resources_root != "", "Empty resources_root is not legal, try '.' instead!")

    annotation_processor_properties = (
        create_annotation_processor_properties(
            ctx,
            ctx.attrs.plugins + ctx.attrs.non_exec_dep_plugins_deprecated,
            ctx.attrs.annotation_processors,
            ctx.attrs.annotation_processor_params,
            ctx.attrs.annotation_processor_deps,
        )
        if run_annotation_processors
        else None
    )
    plugin_params = create_plugin_params(ctx, ctx.attrs.plugins + ctx.attrs.non_exec_dep_plugins_deprecated) if run_annotation_processors else None
    manifest_file = ctx.attrs.manifest_file
    source_level, target_level = get_java_version_attributes(ctx)

    outputs = None
    common_compile_kwargs = None
    has_srcs = bool(srcs) or bool(additional_compiled_srcs)
    if has_srcs or resources or manifest_file:
        abi_generation_mode = override_abi_generation_mode or get_abi_generation_mode(ctx.attrs.abi_generation_mode)

        common_compile_kwargs = {
            "abi_generation_mode": abi_generation_mode,
            "additional_classpath_entries": additional_classpath_entries,
            "additional_compiled_srcs": additional_compiled_srcs,
            "annotation_processor_properties": annotation_processor_properties,
            "custom_jdk_info": custom_jdk_info,
            "debug_port": getattr(ctx.attrs, "debug_port", None),
            "deps": first_order_deps,
            "enable_depfiles": getattr(ctx.attrs, "enable_depfiles", True),
            "javac_tool": derive_javac(ctx.attrs.javac) if getattr(ctx.attrs, "javac", None) else None,
            "manifest_file": manifest_file,
            "remove_classes": ctx.attrs.remove_classes,
            "required_for_source_only_abi": ctx.attrs.required_for_source_only_abi,
            "resources": resources,
            "resources_root": resources_root,
            "source_level": source_level,
            "source_only_abi_deps": ctx.attrs.source_only_abi_deps,
            "srcs": srcs,
            "target_level": target_level,
        }

        # The outputs of validation_deps need to be added as hidden arguments
        # to an action for the validation_deps targets to be built and enforced.
        extra_arguments = cmd_args(
            ctx.attrs.extra_arguments + extra_arguments,
            hidden = validation_deps_outputs or [],
        )

        outputs = compile_to_jar(ctx, plugin_params = plugin_params, extra_arguments = extra_arguments, **common_compile_kwargs)

    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    if common_compile_kwargs and srcs and not ctx.attrs._is_building_android_binary:
        extra_sub_targets = _nullsafe_subtarget(ctx, extra_sub_targets, common_compile_kwargs)
        if not java_toolchain.is_bootstrap_toolchain:
            extra_sub_targets = _semanticdb_subtarget(ctx, extra_sub_targets, java_toolchain, common_compile_kwargs)

    gwt_output = None
    if (srcs or resources) and not java_toolchain.is_bootstrap_toolchain and not ctx.attrs._is_building_android_binary:
        gwt_output = ctx.actions.declare_output("gwt_module/{}.jar".format(ctx.label.name), has_content_based_path = False)
        entries = []

        if srcs or resources:
            entries.append(_copy_resources(ctx.actions, "gwt_module", java_toolchain, ctx.label.package, srcs + resources, resources_root, False))
        if outputs and outputs.annotation_processor_output:
            entries.append(outputs.annotation_processor_output)

        gwt_cmd_args = cmd_args(
            java_toolchain.jar_builder,
            "--entries-to-jar",
            ctx.actions.write("gwt_entries.txt", entries, has_content_based_path = False),
            "--output",
            gwt_output.as_output(),
            hidden = entries,
        )

        ctx.actions.run(gwt_cmd_args, category = "gwt_module")

    all_generated_sources = list(generated_sources)
    if outputs and outputs.annotation_processor_output:
        all_generated_sources.append(outputs.annotation_processor_output)

    if len(all_generated_sources) == 1:
        extra_sub_targets = extra_sub_targets | {
            "generated_sources": [
                DefaultInfo(default_output = all_generated_sources[0]),
            ]
        }

    class_to_src_map, sources_jar, class_to_src_map_sub_targets = get_class_to_source_map_info(
        ctx,
        outputs = outputs,
        deps = ctx.attrs.deps + deps_query + ctx.attrs.exported_deps,
        generate_sources_jar = True,
        class_to_src_map_deps = getattr(ctx.attrs, "class_to_src_map_deps", []),
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
        generated_sources = all_generated_sources,
        has_srcs = has_srcs,
        sources_jar = sources_jar,
        gwt_module = gwt_output,
        preprocessed_library = outputs.preprocessed_library if outputs else None,
        used_jars_json = outputs.used_jars_json if outputs else None,
    )

    validation_specs = get_attrs_validation_specs(ctx)
    if hasattr(ctx.attrs, "validation_specs"):
        validation_specs.extend([ValidationSpec(name = name, validation_result = result) for name, result in ctx.attrs.validation_specs.items()])

    validation_info = ValidationInfo(validations = validation_specs) if validation_specs else None

    default_info = get_default_info(
        ctx.actions,
        java_toolchain,
        outputs,
        java_packaging_info,
        extra_sub_targets,
    )
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

def _nullsafe_subtarget(ctx: AnalysisContext, extra_sub_targets: dict, common_compile_kwargs: dict):
    nullsafe_info = get_nullsafe_info(ctx)
    if nullsafe_info:
        compile_to_jar(
            ctx,
            actions_identifier = "nullsafe",
            plugin_params = nullsafe_info.plugin_params,
            extra_arguments = nullsafe_info.extra_arguments,
            is_creating_subtarget = True,
            **common_compile_kwargs,
        )

        extra_sub_targets = extra_sub_targets | {
            "nullsafex-json": [
                DefaultInfo(default_output = nullsafe_info.output),
            ]
        }
    return extra_sub_targets

def _semanticdb_subtarget(ctx: AnalysisContext, extra_sub_targets: dict, java_toolchain: JavaToolchainInfo, common_compile_kwargs: dict):
    semanticdb_javac_plugin = java_toolchain.semanticdb_javac
    sourceroot = java_toolchain.semanticdb_sourceroot
    if not sourceroot or not semanticdb_javac_plugin:
        return extra_sub_targets
    sourceroot_args = cmd_args(sourceroot, format = "-sourceroot:{}")
    semanticdb_output = ctx.actions.declare_output("semanticdb", dir = True, has_content_based_path = False)
    targetroot_args = cmd_args(semanticdb_output.as_output(), format = "-targetroot:{}")
    semanticdb_plugin_params = create_plugin_params(
        ctx,
        [(semanticdb_javac_plugin, cmd_args(sourceroot_args, targetroot_args))],
    )
    compile_to_jar(
        ctx,
        actions_identifier = "semanticdb_javac",
        plugin_params = semanticdb_plugin_params,
        extra_arguments = None,
        is_creating_subtarget = True,
        **common_compile_kwargs,
    )
    extra_sub_targets = extra_sub_targets | {
        "semanticdb": [
            DefaultInfo(default_output = semanticdb_output),
        ]
    }
    return extra_sub_targets

java_library = prelude_rule(
    name = "java_library",
    impl = java_library_impl,
    docs = """
        A `java_library()` rule defines a set of Java
        files that can be compiled together. The main output of a
         `java_library()` rule is a single JAR file containing all
        of the compiled class files, as well as the static files specified in
        the `resources` argument.
    """,
    examples = """
        ```
        # A rule that compiles a single .java file.
        java_library(
          name = 'JsonUtil',
          srcs = ['JsonUtil.java'],
          deps = [
            '//third_party/guava:guava',
            '//third_party/jackson:jackson',
          ],
        )

        # A rule that compiles all of the .java files under the directory in
        # which the rule is defined using glob(). It also excludes an
        # individual file that may have additional dependencies, so it is
        # compiled by a separate rule.
        java_library(
          name = 'messenger',
          srcs = glob(['**/*.java'], excludes = ['MessengerModule.java']),
          deps = [
            '//src/com/facebook/base:base',
            '//third_party/guava:guava',
          ],
        )

        java_library(
          name = 'MessengerModule',
          srcs = ['MessengerModule.java'],
          deps = [
            '//src/com/facebook/base:base',
            '//src/com/google/inject:inject',
            '//third_party/guava:guava',
            '//third_party/jsr-330:jsr-330',
          ],
        )

        # A rule that builds a library with both relative and
        # fully-qualified deps.
        java_library(
          name = 'testutil',
          srcs = glob(['tests/**/*.java'], excludes = 'tests/**/*Test.java'),
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
                The set of `.java` files to compile for this rule.
                 If any of the files in this list end in `.src.zip`,
                 then the entries in the ZIP file that end in `.java` will be
                 included as ordinary inputs to compilation. This is common when using
                 a `genrule()` to auto-generate some Java source code that
                 needs to be compiled with some hand-written Java code.
            """,
            ),
        }
        | jvm_common.resources_arg()
        | {
            "concat_resources": attrs.bool(
                default = False,
                doc = """
                Use parallel compression and concatenation of intermediary jars to speed up jar time generation.
            """,
            ),
            "deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                Rules (usually other `java_library` rules) that are used to
                 generate the classpath required to compile this `java_library`.
            """,
            ),
            "extra_arguments": attrs.list(
                attrs.string(),
                default = [],
                doc = """
                List of additional arguments to pass into the Java compiler. These
                 arguments follow the ones specified in `.buckconfig`.
            """,
            ),
            "java_version": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                Equivalent to setting both `source` and `target`  to the given value. Setting this and `source` or `target` (or both!) is an error.
            """,
            ),
            "source": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                Specifies the version of Java (as a string) to interpret source
                 files as.
                 Overrides the value in "source\\_level" in the "java" section
                 of `.buckconfig`.
            """,
            ),
            "target": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                Specifies the version of Java (as a string) for which to
                 generate code.
                 Overrides the value in "target\\_level" in the "java" section
                 of `.buckconfig`.
            """,
            ),
        }
        | jvm_common.annotation_processors()
        | jvm_common.remove_classes_arg()
        | jvm_common.exported_deps()
        | jvm_common.provided_deps()
        | jvm_common.exported_provided_deps()
        | jvm_common.abi_generation_mode()
        | jvm_common.source_only_abi_deps()
        | jvm_common.required_for_source_only_abi()
        | jvm_common.plugins()
        | jvm_common.multi_release_jar()
        | jvm_common.javac()
        | jvm_common.classic_java_content_based_paths()
        | {
            "class_to_src_map_deps": attrs.list(attrs.dep(), default = []),
            "jar_postprocessor": attrs.option(attrs.exec_dep(), default = None),
            "manifest_file": attrs.option(attrs.source(), default = None),
            "maven_coords": attrs.option(attrs.string(), default = None),
            "min_sdk_version": attrs.default_only(attrs.option(attrs.int(), default = min_sdk_version())),
            "proguard_config": attrs.option(attrs.source(), default = None),
            "resources_root": attrs.option(attrs.string(), default = None),
            "runtime_deps": attrs.list(attrs.dep(), default = []),
            VALIDATION_DEPS_ATTR_NAME: attrs.set(attrs.dep(), sorted = True, default = []),
            "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
            "_dex_toolchain": toolchains_common.dex(),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_is_building_android_binary": is_building_android_binary_attr(),
            "_java_toolchain": toolchains_common.java(),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | validation_common.attrs_validators_arg()
    ),
)
