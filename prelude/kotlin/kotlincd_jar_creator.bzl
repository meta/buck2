# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//java:compiler_depth.bzl", "compiler_depth_for_action", "max_compiler_depth")
load(
    "@prelude//java:java_providers.bzl",
    "ClasspathSnapshotGranularity",
    "JavaCompileOutputs",  # @unused Used as a type
    "JavaCompilingDepsTSet",  # @unused Used as a type
    "JavaLibraryInfo",
    "generate_java_classpath_snapshot",
    "get_compiling_deps_tset",
    "make_compile_outputs",
)
load("@prelude//java:java_resources.bzl", "get_resources_map")
load("@prelude//java:java_toolchain.bzl", "AbiGenerationMode", "DepFiles", "JavaToolchainInfo")
load(
    "@prelude//java/plugins:java_annotation_processor.bzl",
    "AnnotationProcessorProperties",  # @unused Used as type
)
load(
    "@prelude//java/plugins:java_plugin.bzl",
    "PluginParams",  # @unused Used as type
)
load(
    "@prelude//java/utils:java_utils.bzl",
    "CustomJdkInfo",  # @unused Used as type
)
load(
    "@prelude//jvm:cd_jar_creator_util.bzl",
    "BuildMode",
    "OutputPaths",
    "TargetType",
    "base_qualified_name",
    "declare_prefixed_output",
    "define_output_paths",
    "encode_command",
    "generate_abi_jars",
    "get_source_only_abi_compiling_deps",
    "prepare_cd_exe",
    "prepare_final_jar",
    "setup_dep_files",
)
load("@prelude//kotlin:kotlin_toolchain.bzl", "KotlinToolchainInfo")
load("@prelude//kotlin:kotlin_utils.bzl", "get_kotlinc_compatible_target")
load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:utils.bzl", "map_idx")

def create_jar_artifact_kotlincd(
    actions: AnalysisActions,
    actions_identifier: [str, None],
    abi_generation_mode: [AbiGenerationMode, None],
    java_toolchain: JavaToolchainInfo,
    kotlin_toolchain: KotlinToolchainInfo,
    label: Label,
    srcs: list[Artifact],
    remove_classes: list[str],
    resources: list[Artifact],
    resources_root: [str, None],
    annotation_processor_properties: AnnotationProcessorProperties,
    plugin_params: [PluginParams, None],
    manifest_file: Artifact | None,
    source_level: int,
    target_level: int,
    deps: list[Dependency],
    required_for_source_only_abi: bool,
    source_only_abi_deps: list[Dependency],
    extra_arguments: cmd_args,
    additional_classpath_entries: JavaCompilingDepsTSet | None,
    bootclasspath_entries: list[Artifact],
    bootclasspath_snapshot_entries: list[Artifact],
    custom_jdk_info: CustomJdkInfo | None,
    is_building_android_binary: bool,
    friend_paths: list[Dependency],
    kotlin_compiler_plugins: list[(Dependency, dict[str, [str, cmd_args]])],
    extra_kotlinc_arguments: list,
    incremental: bool,
    language_version: str,
    is_creating_subtarget: bool = False,
    optional_dirs: list[OutputArtifact] = [],
    jar_postprocessor: [RunInfo, None] = None,
    debug_port: [int, None] = None,
    enable_depfiles: [bool, None] = True,
    track_files_which_skipped_compilation: bool = False,
) -> (JavaCompileOutputs, Artifact, dict[str, Artifact]):
    resources_map = get_resources_map(
        java_toolchain = java_toolchain,
        package = label.package,
        resources = resources,
        resources_root = resources_root,
    )

    expect(abi_generation_mode != AbiGenerationMode("source"), "abi_generation_mode: source is not supported in kotlincd")
    if abi_generation_mode == AbiGenerationMode("source_only") and language_version < "2.0":
        fail(
            'source_only ABI generation is not supported with K1 (language_version {}). Either upgrade to K2 (language_version >= 2.0) or use abi_generation_mode = "class".'.format(
                language_version
            )
        )
    actual_abi_generation_mode = abi_generation_mode or AbiGenerationMode("class") if srcs else AbiGenerationMode("none")
    kosabi_applicability_cell_root = (
        label.cell_root
        if not is_creating_subtarget and actual_abi_generation_mode == AbiGenerationMode("source_only") and kotlin_toolchain.kosabi_applicability_plugin != None
        else None
    )
    output_paths = define_output_paths(
        actions,
        actions_identifier,
        label,
        uses_content_based_paths = True,
        emit_compiler_class_to_source_map = not is_creating_subtarget,
    )

    # Only create class-abi inline for class-mode targets. For source_only targets,
    # the fallback in cd_jar_creator_util.bzl (create_abi) handles class-abi generation
    # for subtargets. This keeps the kotlincd action key config-invariant. See T264990210.
    should_create_class_abi = (
        not is_creating_subtarget and actual_abi_generation_mode == AbiGenerationMode("class") and kotlin_toolchain.jvm_abi_gen_plugin != None
    )
    if should_create_class_abi:
        class_abi_jar = declare_prefixed_output(actions, actions_identifier, "class-abi.jar", has_content_based_path = True)
        class_abi_output_dir = declare_prefixed_output(actions, actions_identifier, "class_abi_dir", has_content_based_path = True, dir = True)
        jvm_abi_gen = cmd_args(output_paths.jar.as_output(), format = "{}/jvm-abi-gen.jar", parent = 1)
        should_use_jvm_abi_gen = True
    else:
        class_abi_jar = None
        class_abi_output_dir = None
        jvm_abi_gen = None
        should_use_jvm_abi_gen = False

    expect(incremental == False or kosabi_applicability_cell_root == None, "Kosabi applicability requires a full compilation")
    should_kotlinc_run_incrementally = kotlin_toolchain.enable_incremental_compilation and incremental
    incremental_state_dir = declare_prefixed_output(actions, actions_identifier, "incremental_state", has_content_based_path = True, dir = True)
    incremental_metadata_ignored_inputs_tag = actions.artifact_tag()

    compiling_deps_tset = get_compiling_deps_tset(actions, deps, [additional_classpath_entries] if additional_classpath_entries else [])
    friend_abi_compiler_depths = {}
    for friend in friend_paths:
        info = friend.get(JavaLibraryInfo)
        if info and info.library_output:
            friend_abi_compiler_depths[info.library_output.abi] = info.library_output.abi_compiler_depth
    full_library_compiler_depth = compiler_depth_for_action(
        label, "kotlincd_jar", compiling_deps_tset, identifier = actions_identifier, additional_depth = max_compiler_depth(friend_abi_compiler_depths.values())
    )

    # Compute the reduced SO-ABI classpath for applicability checking during
    # library builds. The applicability plugin needs to know which deps will be
    # available during source-only-abi generation (only deps with
    # required_for_source_only_abi=True or in source_only_abi_deps).
    source_only_abi_applicability_classpath = cmd_args()
    so_abi_deps = None
    if actual_abi_generation_mode == AbiGenerationMode("source_only"):
        so_abi_deps = get_source_only_abi_compiling_deps(actions, compiling_deps_tset, source_only_abi_deps)
        source_only_abi_applicability_classpath = cmd_args(so_abi_deps.project_as_args("source_only_abi_jars"))

    track_class_usage = enable_depfiles and kotlin_toolchain.track_class_usage_plugin != None

    if track_files_which_skipped_compilation:
        files_which_skipped_compilation = declare_prefixed_output(actions, actions_identifier, "files_which_skipped_compilation", has_content_based_path = True)
        jvm_abi_files_which_skipped_compilation = (
            declare_prefixed_output(actions, actions_identifier, "jvm_abi_files_which_skipped_compilation", has_content_based_path = True)
            if should_use_jvm_abi_gen
            else None
        )
    else:
        files_which_skipped_compilation = None
        jvm_abi_files_which_skipped_compilation = None

    define_kotlincd_action = partial(
        _define_kotlincd_action,
        actions,
        java_toolchain,
        kotlin_toolchain,
        srcs,
        should_create_class_abi,
        class_abi_jar,
        jvm_abi_gen,
        class_abi_output_dir,
        optional_dirs,
        track_class_usage,
        compiling_deps_tset,
        debug_port,
        incremental_metadata_ignored_inputs_tag,
        jar_postprocessor,
        files_which_skipped_compilation,
        jvm_abi_files_which_skipped_compilation,
    )

    library_classpath_jars_tag = actions.artifact_tag()
    command_builder = _command_builder(
        label = label,
        srcs = srcs,
        remove_classes = remove_classes,
        annotation_processor_properties = annotation_processor_properties,
        plugin_params = plugin_params,
        manifest_file = manifest_file,
        source_level = source_level,
        target_level = target_level,
        compiling_deps_tset = compiling_deps_tset,
        bootclasspath_entries = bootclasspath_entries,
        system_image = custom_jdk_info.system_image if custom_jdk_info else None,
        abi_generation_mode = actual_abi_generation_mode,
        resources_map = resources_map,
        extra_arguments = extra_arguments,
        use_abi_dirs = bool(not is_creating_subtarget and srcs and track_class_usage and kotlin_toolchain.dep_files == DepFiles("per_class")),
    )

    # this is required for the Kotlin compiler to be able to use jspecify annotations
    extra_kotlinc_arguments = ["-Xjspecify-annotations=strict"] + extra_kotlinc_arguments

    kotlin_extra_params = _encode_kotlin_extra_params(
        kotlin_toolchain = kotlin_toolchain,
        kotlin_compiler_plugins = kotlin_compiler_plugins,
        extra_kotlinc_arguments = extra_kotlinc_arguments,
        bootclasspath_entries = bootclasspath_entries,
        bootclasspath_snapshot_entries = bootclasspath_snapshot_entries,
        friend_paths = [friend_path.library_output.abi for friend_path in map_idx(JavaLibraryInfo, friend_paths) if friend_path.library_output],
        target_level = target_level,
        should_use_jvm_abi_gen = should_use_jvm_abi_gen,
        kosabi_applicability_cell_root = kosabi_applicability_cell_root,
        should_kotlinc_run_incrementally = should_kotlinc_run_incrementally,
        incremental_state_dir = incremental_state_dir,
        language_version = language_version,
        source_only_abi_applicability_classpath = source_only_abi_applicability_classpath,
    )

    library_command_builder = command_builder(
        kotlin_extra_params = kotlin_extra_params,
        provide_classpath_snapshot = should_kotlinc_run_incrementally,
        incremental_metadata_ignored_inputs_tag = incremental_metadata_ignored_inputs_tag if should_kotlinc_run_incrementally else None,
    )
    command = library_command_builder(
        build_mode = BuildMode("LIBRARY"),
        target_type = TargetType("library"),
        output_paths = output_paths,
        classpath_jars_tag = library_classpath_jars_tag,
        source_only_abi_compiling_deps = None,
        track_class_usage = track_class_usage,
    )
    proto, used_jars_json = define_kotlincd_action(
        category_prefix = "",
        actions_identifier = actions_identifier,
        encoded_command = command,
        qualified_name = base_qualified_name(label),
        output_paths = output_paths,
        classpath_jars_tag = library_classpath_jars_tag,
        abi_dir = class_abi_output_dir if should_create_class_abi else None,
        target_type = TargetType("library"),
        is_creating_subtarget = is_creating_subtarget,
        incremental_state_dir = incremental_state_dir,
        should_action_run_incrementally = should_kotlinc_run_incrementally,
    )

    final_jar_output = prepare_final_jar(
        actions = actions,
        actions_identifier = actions_identifier,
        output_paths = output_paths,
        output = None,
        additional_compiled_srcs = None,
        jar_builder = java_toolchain.jar_builder,
        jar_postprocessor = jar_postprocessor,
        jar_postprocessor_runner = java_toolchain.postprocessor_runner[RunInfo] if java_toolchain.postprocessor_runner else None,
        zip_scrubber = java_toolchain.zip_scrubber,
        uses_content_based_paths = True,
        postprocessor_merged_into_compile_action = jar_postprocessor != None,
    )

    if not is_creating_subtarget:
        source_only_friend_abis = get_source_only_friend_path_abis(friend_paths, source_only_abi_deps)
        kotlin_extra_params_builder = partial(
            _encode_kotlin_extra_params,
            kotlin_toolchain = kotlin_toolchain,
            kotlin_compiler_plugins = kotlin_compiler_plugins,
            extra_kotlinc_arguments = extra_kotlinc_arguments,
            bootclasspath_entries = bootclasspath_entries,
            bootclasspath_snapshot_entries = bootclasspath_snapshot_entries,
            friend_paths = source_only_friend_abis,
            target_level = target_level,
            should_use_jvm_abi_gen = should_use_jvm_abi_gen,
            kosabi_applicability_cell_root = None,
            should_kotlinc_run_incrementally = False,
            incremental_state_dir = None,
            language_version = language_version,
            source_only_abi_applicability_classpath = cmd_args(),
        )

        # kotlincd does not support source abi
        class_abi, _, source_only_abi, classpath_abi, classpath_abi_dir, abi_compiler_depth = generate_abi_jars(
            actions = actions,
            actions_identifier = actions_identifier,
            label = label,
            abi_generation_mode = actual_abi_generation_mode,
            additional_compiled_srcs = None,
            is_building_android_binary = is_building_android_binary,
            class_abi_generator = java_toolchain.class_abi_generator,
            final_jar = final_jar_output.final_jar,
            compiling_deps_tset = compiling_deps_tset,
            source_only_abi_deps = source_only_abi_deps,
            class_abi_jar = class_abi_jar,
            class_abi_output_dir = class_abi_output_dir,
            track_class_usage = True,
            encode_abi_command = command_builder,
            define_action = define_kotlincd_action,
            uses_content_based_paths = True,
            kotlin_extra_params_builder = kotlin_extra_params_builder,
            source_only_abi_compiling_deps = so_abi_deps,
            compiler_category = "kotlincd_jar",
            full_library_compiler_depth = full_library_compiler_depth,
            additional_compiler_depth = max_compiler_depth([friend_abi_compiler_depths[abi] for abi in source_only_friend_abis]),
        )
        abi_jar_snapshot = generate_java_classpath_snapshot(
            actions, java_toolchain.cp_snapshot_generator, ClasspathSnapshotGranularity("CLASS_MEMBER_LEVEL"), classpath_abi, actions_identifier
        )
        tracking_outputs = {}
        if files_which_skipped_compilation:
            tracking_outputs["files_which_skipped_compilation"] = files_which_skipped_compilation
        if jvm_abi_files_which_skipped_compilation:
            tracking_outputs["jvm_abi_files_which_skipped_compilation"] = jvm_abi_files_which_skipped_compilation
        return (
            make_compile_outputs(
                full_library = final_jar_output.final_jar,
                preprocessed_library = final_jar_output.preprocessed_jar,
                class_abi = class_abi,
                source_only_abi = source_only_abi,
                classpath_abi = classpath_abi,
                classpath_abi_dir = classpath_abi_dir,
                full_library_compiler_depth = full_library_compiler_depth,
                abi_compiler_depth = abi_compiler_depth,
                required_for_source_only_abi = required_for_source_only_abi,
                annotation_processor_output = output_paths.annotations,
                incremental_state_dir = incremental_state_dir,
                abi_jar_snapshot = abi_jar_snapshot,
                used_jars_json = used_jars_json,
                compiler_class_to_source_map = output_paths.compiler_class_to_source_map,
            ),
            proto,
            tracking_outputs,
        )
    else:
        full_jar_snapshot = generate_java_classpath_snapshot(
            actions, java_toolchain.cp_snapshot_generator, ClasspathSnapshotGranularity("CLASS_MEMBER_LEVEL"), final_jar_output.final_jar, actions_identifier
        )
        return (
            make_compile_outputs(
                full_library = final_jar_output.final_jar,
                preprocessed_library = final_jar_output.preprocessed_jar,
                required_for_source_only_abi = required_for_source_only_abi,
                annotation_processor_output = output_paths.annotations,
                abi_jar_snapshot = full_jar_snapshot,
                full_library_compiler_depth = full_library_compiler_depth,
                abi_compiler_depth = full_library_compiler_depth,
                used_jars_json = used_jars_json,
            ),
            proto,
            {},
        )

def get_source_only_friend_path_abis(friend_paths: list[Dependency], source_only_abi_deps: list[Dependency]) -> list[Artifact]:
    """Returns friend ABI jars restricted to the source-only ABI classpath."""
    if not friend_paths:
        return []

    # Friendship grants internal access to a library already on the classpath; it does not add
    # that library to the classpath. Passing an excluded friend's jar to a source-only action
    # therefore only creates an unnecessary build dependency.
    # Match the reduced classpath by artifact, including aliases of the same library.
    explicit_abis = set()
    for dep in source_only_abi_deps:
        info = dep.get(JavaLibraryInfo)
        if info and info.library_output:
            explicit_abis.add(info.library_output.abi)

    friends = []
    for dep in friend_paths:
        info = dep.get(JavaLibraryInfo)
        if not info or not info.library_output:
            continue
        entry = info.library_output
        if entry.required_for_source_only_abi or entry.abi in explicit_abis:
            friends.append(entry.abi)
    return friends

def _encode_kotlin_extra_params(
    kotlin_toolchain: KotlinToolchainInfo,
    kotlin_compiler_plugins: list[(Dependency, dict[str, [str, cmd_args]])],
    extra_kotlinc_arguments: list,
    bootclasspath_entries: list[Artifact],
    bootclasspath_snapshot_entries: list[Artifact],
    friend_paths: list[Artifact],
    target_level: int,
    should_use_jvm_abi_gen: bool,
    kosabi_applicability_cell_root,
    should_kotlinc_run_incrementally: bool,
    incremental_state_dir: Artifact | None,
    language_version: str,
    source_only_abi_applicability_classpath: cmd_args = cmd_args(),
):
    kosabiPluginOptionsMap = {}

    if kotlin_toolchain.kosabi_stubs_gen_k2_plugin != None:
        kosabiPluginOptionsMap["kosabi_stubs_gen_k2_plugin"] = kotlin_toolchain.kosabi_stubs_gen_k2_plugin

    if kotlin_toolchain.kosabi_applicability_plugin != None:
        kosabiPluginOptionsMap["kosabi_applicability_plugin"] = kotlin_toolchain.kosabi_applicability_plugin

    if kosabi_applicability_cell_root != None:
        kosabiPluginOptionsMap["kosabi_applicability_cell_root"] = cmd_args(kosabi_applicability_cell_root, delimiter = "")

    if kotlin_toolchain.kosabi_jvm_abi_gen_k2_plugin != None:
        kosabiPluginOptionsMap["kosabi_jvm_abi_gen_k2_plugin"] = kotlin_toolchain.kosabi_jvm_abi_gen_k2_plugin

    return struct(
        extraClassPaths = bootclasspath_entries,
        extraClassPathSnapshots = bootclasspath_snapshot_entries if should_kotlinc_run_incrementally else [],
        standardLibraryClassPath = kotlin_toolchain.kotlin_stdlib[JavaLibraryInfo].library_output.full_library,
        annotationProcessingClassPath = kotlin_toolchain.annotation_processing_jar[JavaLibraryInfo].library_output.full_library,
        jvmAbiGenPlugin = kotlin_toolchain.jvm_abi_gen_plugin,
        kotlinCompilerPlugins = {plugin[DefaultInfo].default_outputs[0]: {"params": plugin_options} for plugin, plugin_options in kotlin_compiler_plugins},
        kosabiPluginOptions = struct(**kosabiPluginOptionsMap),
        friendPaths = friend_paths,
        kotlinHomeLibraries = kotlin_toolchain.kotlin_home_libraries,
        jvmTarget = get_kotlinc_compatible_target(str(target_level)),
        shouldUseJvmAbiGen = should_use_jvm_abi_gen,
        shouldVerifySourceOnlyAbiConstraints = kosabi_applicability_cell_root != None,
        extraKotlincArguments = extra_kotlinc_arguments,
        depTrackerPlugin = kotlin_toolchain.track_class_usage_plugin,
        shouldKotlincRunIncrementally = should_kotlinc_run_incrementally,
        shouldKosabiUseShallowClassUsage = kotlin_toolchain.kosabi_shallow_class_usage,
        kosabiAbiValidationMode = kotlin_toolchain.kosabi_abi_validation_mode,
        incrementalStateDir = incremental_state_dir.as_output() if incremental_state_dir else None,
        languageVersion = language_version,
        javaBinary = cmd_args(kotlin_toolchain.java_binary_for_kotlincd[RunInfo], delimiter = " ") if kotlin_toolchain.java_binary_for_kotlincd else "",
        applicabilityClasspath = source_only_abi_applicability_classpath,
    )

def _command_builder(
    label: Label,
    srcs: list[Artifact],
    remove_classes: list[str],
    annotation_processor_properties: AnnotationProcessorProperties,
    plugin_params: [PluginParams, None],
    manifest_file: Artifact | None,
    source_level: int,
    target_level: int,
    compiling_deps_tset: [JavaCompilingDepsTSet, None],
    bootclasspath_entries: list[Artifact],
    system_image: Artifact | None,
    abi_generation_mode: AbiGenerationMode,
    resources_map: dict[str, Artifact],
    extra_arguments: cmd_args,
    use_abi_dirs: bool,
):
    return partial(
        _encode_kotlin_command,
        label = label,
        srcs = srcs,
        remove_classes = remove_classes,
        annotation_processor_properties = annotation_processor_properties,
        plugin_params = plugin_params,
        manifest_file = manifest_file,
        source_level = source_level,
        target_level = target_level,
        compiling_deps_tset = compiling_deps_tset,
        bootclasspath_entries = bootclasspath_entries,
        system_image = system_image,
        abi_generation_mode = abi_generation_mode,
        resources_map = resources_map,
        extra_arguments = extra_arguments,
        use_abi_dirs = use_abi_dirs,
    )

def _encode_kotlin_command(
    label: Label,
    srcs: list[Artifact],
    remove_classes: list[str],
    annotation_processor_properties: AnnotationProcessorProperties,
    plugin_params: [PluginParams, None],
    manifest_file: Artifact | None,
    source_level: int,
    target_level: int,
    compiling_deps_tset: [JavaCompilingDepsTSet, None],
    bootclasspath_entries: list[Artifact],
    system_image: Artifact | None,
    abi_generation_mode: AbiGenerationMode,
    resources_map: dict[str, Artifact],
    extra_arguments: cmd_args,
    kotlin_extra_params: [struct, None],
    provide_classpath_snapshot: bool,
    use_abi_dirs: bool,
    incremental_metadata_ignored_inputs_tag: ArtifactTag | None = None,
):
    return partial(
        encode_command,
        label = label,
        srcs = srcs,
        remove_classes = remove_classes,
        annotation_processor_properties = annotation_processor_properties,
        plugin_params = plugin_params,
        manifest_file = manifest_file,
        source_level = source_level,
        target_level = target_level,
        compiling_deps_tset = compiling_deps_tset,
        bootclasspath_entries = bootclasspath_entries,
        system_image = system_image,
        abi_generation_mode = abi_generation_mode,
        resources_map = resources_map,
        extra_arguments = extra_arguments,
        kotlin_extra_params = kotlin_extra_params,
        provide_classpath_snapshot = provide_classpath_snapshot,
        use_abi_dirs = use_abi_dirs,
        incremental_metadata_ignored_inputs_tag = incremental_metadata_ignored_inputs_tag,
    )

# buildifier: disable=uninitialized
# buildifier: disable=unused-variable
def _define_kotlincd_action(
    # provided by factory
    actions: AnalysisActions,
    java_toolchain: JavaToolchainInfo,
    kotlin_toolchain: KotlinToolchainInfo,
    srcs: list[Artifact],
    should_create_class_abi: bool,
    class_abi_jar: [Artifact, None],
    jvm_abi_gen: [cmd_args, None],
    class_abi_output_dir: [Artifact, None],
    optional_dirs: list[OutputArtifact],
    track_class_usage: bool,
    compiling_deps_tset: [JavaCompilingDepsTSet, None],
    debug_port: [int, None],
    incremental_metadata_ignored_inputs_tag: ArtifactTag,
    jar_postprocessor: [RunInfo, None],
    files_which_skipped_compilation: [Artifact, None],
    jvm_abi_files_which_skipped_compilation: [Artifact, None],
    # end of factory provided
    category_prefix: str,
    actions_identifier: [str, None],
    encoded_command: struct,
    qualified_name: str,
    output_paths: OutputPaths,
    classpath_jars_tag: ArtifactTag,
    abi_dir: Artifact | None,
    target_type: TargetType,
    is_creating_subtarget: bool = False,
    incremental_state_dir: Artifact | None = None,
    should_action_run_incrementally: bool = False,
):
    compiler = kotlin_toolchain.kotlincd[DefaultInfo].default_outputs[0]
    exe, local_only = prepare_cd_exe(
        qualified_name,
        java = java_toolchain.java[RunInfo],
        class_loader_bootstrapper = kotlin_toolchain.class_loader_bootstrapper,
        compiler = compiler,
        main_class = kotlin_toolchain.kotlincd_main_class,
        worker = kotlin_toolchain.kotlincd_worker[WorkerInfo] if kotlin_toolchain.kotlincd_worker else None,
        remote_worker = kotlin_toolchain.kotlincd_remote_worker[WorkerInfo] if kotlin_toolchain.kotlincd_remote_worker else None,
        target_specified_debug_port = debug_port,
        toolchain_specified_debug_port = kotlin_toolchain.kotlincd_debug_port,
        toolchain_specified_debug_target = kotlin_toolchain.kotlincd_debug_target,
        extra_jvm_args = kotlin_toolchain.kotlincd_jvm_args,
        extra_jvm_args_target = kotlin_toolchain.kotlincd_jvm_args_target,
        java_runtime_version = java_toolchain.java_runtime_version,
    )

    args = cmd_args()
    post_build_params = {}
    if target_type == TargetType("library") and should_create_class_abi:
        post_build_params["shouldCreateClassAbi"] = True
        post_build_params["libraryJar"] = output_paths.jar.as_output()
        post_build_params["abiJar"] = class_abi_jar.as_output()
        post_build_params["jvmAbiGen"] = jvm_abi_gen
        post_build_params["abiOutputDir"] = class_abi_output_dir.as_output()

    if target_type == TargetType("source_abi") or target_type == TargetType("source_only_abi"):
        post_build_params["abiJar"] = output_paths.jar.as_output()
        post_build_params["abiOutputDir"] = abi_dir.as_output()

    if optional_dirs:
        post_build_params["optionalDirs"] = optional_dirs

    if incremental_state_dir:
        post_build_params["incrementalStateDir"] = incremental_state_dir.as_output()

    if target_type == TargetType("library") and files_which_skipped_compilation:
        post_build_params["filesWhichSkippedCompilation"] = files_which_skipped_compilation.as_output()
    if target_type == TargetType("library") and jvm_abi_files_which_skipped_compilation:
        post_build_params["jvmAbiFilesWhichSkippedCompilation"] = jvm_abi_files_which_skipped_compilation.as_output()

    if jar_postprocessor and target_type == TargetType("library"):
        if "libraryJar" not in post_build_params:
            post_build_params["libraryJar"] = output_paths.jar.as_output()
        post_build_params["postProcessorCmd"] = cmd_args(jar_postprocessor, delimiter = " ")

    dep_files = {}
    used_jars_json_output = None
    if (
        not is_creating_subtarget
        and srcs
        and (kotlin_toolchain.dep_files == DepFiles("per_jar") or kotlin_toolchain.dep_files == DepFiles("per_class"))
        and track_class_usage
    ):
        used_classes_json_outputs = [
            cmd_args(output_paths.jar.as_output(), format = "{}/used-classes.json", parent = 1),
            cmd_args(output_paths.jar.as_output(), format = "{}/kotlin-used-classes.json", parent = 1),
        ]
        if target_type == TargetType("library"):
            used_jars_json_output = declare_prefixed_output(actions, actions_identifier, "jar/used-jars.json", has_content_based_path = True)
        if should_action_run_incrementally and kotlin_toolchain.dep_files == DepFiles("per_class") and compiling_deps_tset:
            # The combined projection is excluded from incremental metadata to avoid
            # enumerating ABI directories; the incremental compiler still needs the jars.
            args.add(classpath_jars_tag.tag_artifacts(cmd_args(hidden = compiling_deps_tset.project_as_args("args_for_compiling", ordering = "topological"))))
        setup_dep_files(
            actions,
            actions_identifier,
            post_build_params,
            classpath_jars_tag,
            used_classes_json_outputs,
            used_jars_json_output,
            uses_content_based_paths = True,
        )

        dep_files["classpath_jars"] = classpath_jars_tag

    kotlin_build_command = struct(
        buildCommand = encoded_command,
        postBuildParams = post_build_params,
    )

    proto = declare_prefixed_output(actions, actions_identifier, "jar_command.proto.json", has_content_based_path = True)
    dep_file_fingerprints = []
    if dep_files:
        proto, fingerprint = actions.write_json(
            proto,
            kotlin_build_command,
            dep_files_fingerprint_using_canonical_paths = True,
        )
        proto_for_args = classpath_jars_tag.tag_artifacts(proto)
        dep_file_fingerprints.append(fingerprint)
    else:
        proto_for_args = actions.write_json(proto, kotlin_build_command, with_inputs = True)

    args.add(
        "--action-id",
        qualified_name,
        "--command-file",
        proto_for_args,
    )

    if should_action_run_incrementally:
        args.add(
            "--incremental-config-file",
            _create_incremental_config(actions, actions_identifier, kotlin_build_command, kotlin_toolchain.kotlin_version),
        )

    incremental_run_params = (
        {
            "incremental_metadata_ignore_tags": [incremental_metadata_ignored_inputs_tag],
            "metadata_env_var": "ACTION_METADATA",
            "metadata_path": "action_metadata.json",
            "no_outputs_cleanup": True,
        }
        if should_action_run_incrementally
        else {}
    )
    actions.run(
        args,
        env = {
            "BUCK_CLASSPATH": compiler,
            "JAVACD_ABSOLUTE_PATHS_ARE_RELATIVE_TO_CWD": "1",
        },
        category = "{}kotlincd_jar".format(category_prefix),
        identifier = actions_identifier,
        dep_files = dep_files,
        dep_file_fingerprints = dep_file_fingerprints,
        allow_dep_file_cache_upload = True,
        allow_cache_upload = True,
        exe = exe,
        local_only = local_only,
        low_pass_filter = False,
        weight = 2,
        error_handler = kotlin_toolchain.kotlin_error_handler,
        **incremental_run_params,
    )
    return proto, used_jars_json_output

def _create_incremental_config(actions: AnalysisActions, actions_identifier: [str, None], kotlin_build_command: struct, kotlin_version: str):
    incremental_meta_data_output = declare_prefixed_output(actions, actions_identifier, "incremental_config.json", has_content_based_path = True)
    incremental_meta_data = struct(
        version = 3,
        track_class_usage = kotlin_build_command.buildCommand.baseJarCommand.trackClassUsage,
        should_use_jvm_abi_gen = kotlin_build_command.buildCommand.kotlinExtraParams.shouldUseJvmAbiGen,
        extra_kotlinc_arguments = kotlin_build_command.buildCommand.kotlinExtraParams.extraKotlincArguments,
        kotlin_version = kotlin_version,
    )
    return actions.write_json(incremental_meta_data_output, incremental_meta_data, with_inputs = True)
