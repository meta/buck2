# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//java:compiler_depth.bzl",
    "JavaCompilerDepth",  # @unused Used as a type
    "compiler_depth_for_action",
    "max_compiler_depth",
)
load(
    "@prelude//java:java_providers.bzl",
    "JavaClasspathEntry",
    "JavaCompilingDepsTSet",  # @unused Used as type
    "JavaLibraryInfo",
    "create_abi",
)
load("@prelude//java:java_toolchain.bzl", "AbiGenerationMode", "JavaToolchainInfo", "unsafe_memory_access_jvm_args")
load("@prelude//java/plugins:java_annotation_processor.bzl", "AnnotationProcessorProperties")  # @unused Used as type
load(
    "@prelude//java/plugins:java_plugin.bzl",
    "PluginParams",  # @unused Used as type
)
load("@prelude//java/utils:java_utils.bzl", "declare_prefixed_name")
load("@prelude//utils:expect.bzl", "expect")

def declare_prefixed_output(actions: AnalysisActions, prefix: [str, None], output: str, has_content_based_path: bool, dir: bool = False) -> Artifact:
    return actions.declare_output(declare_prefixed_name(output, prefix), dir = dir, has_content_based_path = has_content_based_path)

# The library and the toolchain can both set a specific abi generation
# mode. The toolchain's setting is effectively the "highest" form of abi
# that the toolchain supports and then the same for the target and we will choose
# the "highest" that both support.
def _resolve_abi_generation_mode(abi_generation_mode: [AbiGenerationMode, None], java_toolchain: JavaToolchainInfo) -> AbiGenerationMode:
    if abi_generation_mode == None:
        return java_toolchain.abi_generation_mode
    for mode in [AbiGenerationMode("none"), AbiGenerationMode("class"), AbiGenerationMode("source"), AbiGenerationMode("source_only")]:
        if mode in (java_toolchain.abi_generation_mode, abi_generation_mode):
            return mode
    fail("resolving abi generation mode failed. had `{}` and `{}`".format(java_toolchain.abi_generation_mode, abi_generation_mode))

def get_abi_generation_mode(
    abi_generation_mode: [AbiGenerationMode, None],
    java_toolchain: JavaToolchainInfo,
    srcs: list[Artifact],
    annotation_processor_properties: AnnotationProcessorProperties,
) -> AbiGenerationMode:
    resolved_mode = AbiGenerationMode("none") if not srcs else _resolve_abi_generation_mode(abi_generation_mode, java_toolchain)
    if resolved_mode == AbiGenerationMode("source_only"):

        def plugins_support_source_only_abi():
            for ap in annotation_processor_properties.annotation_processors:
                if ap.affects_abi and not ap.supports_source_only_abi:
                    return False
            return True

        if not plugins_support_source_only_abi():
            resolved_mode = AbiGenerationMode("source")
    return resolved_mode

# We need to construct a complex protobuf message. We do it by constructing
# a bunch of nested structs and then use write_json to get a json-encoded
# protobuf message.
#
# The definition is in xplat/toolchains/android/sdk/src/com/facebook/buck/cd/resources/proto/javacd.proto
# and xplat/toolchains/android/sdk/src/com/facebook/buck/cd/resources/proto/kotlincd.proto and is, sadly,
# poorly documented.

# Our protobuf format mostly encodes paths in RelPath/AbsPath structs with a single "path" field.
# Note that we don't actually use abspath and instead enable JAVACD_ABSOLUTE_PATHS_ARE_RELATIVE_TO_CWD
TargetType = enum("library", "source_abi", "source_only_abi")
BuildMode = enum("LIBRARY", "ABI")

def encode_abi_generation_mode(mode: AbiGenerationMode) -> str:
    return {
        AbiGenerationMode("none"): "NONE",
        AbiGenerationMode("class"): "CLASS",
        AbiGenerationMode("source"): "SOURCE",
        AbiGenerationMode("source_only"): "SOURCE_ONLY",
    }[mode]

def encode_target_type(target_type: TargetType) -> str:
    if target_type == TargetType("library"):
        return "LIBRARY"
    if target_type == TargetType("source_abi"):
        return "SOURCE_ABI"
    if target_type == TargetType("source_only_abi"):
        return "SOURCE_ONLY_ABI"
    fail()

OutputPaths = record(
    jar = Artifact,
    annotations = Artifact | None,
    compiler_class_to_source_map = field(Artifact | None, default = None),
)

def qualified_name_with_subtarget(label: Label) -> str:
    if label.sub_target:
        return "{}:{}[{}]".format(label.path, label.name, label.sub_target[0])
    return "{}:{}".format(label.path, label.name)

# Converted to str so that we get the right result when written as json.
def base_qualified_name(label: Label) -> str:
    return "{}:{}".format(label.path, label.name)

def get_qualified_name(label: Label, target_type: TargetType) -> str:
    # These should match the names for subtargets in java_library.bzl
    return {
        TargetType("library"): base_qualified_name(label),
        TargetType("source_abi"): base_qualified_name(label) + "[source-abi]",
        TargetType("source_only_abi"): base_qualified_name(label) + "[source-only-abi]",
    }[target_type]

def define_output_paths(
    actions: AnalysisActions,
    prefix: [str, None],
    label: Label,
    uses_content_based_paths: bool,
    declare_annotations: bool = True,
    emit_compiler_class_to_source_map: bool = False,
) -> OutputPaths:
    # currently, javacd requires that at least some outputs are in the root
    # output dir. so we put all of them there. If javacd is updated we
    # could consolidate some of these into one subdir.
    return OutputPaths(
        jar = declare_prefixed_output(actions, prefix, "jar/{}.jar".format(label.name), uses_content_based_paths),
        compiler_class_to_source_map = declare_prefixed_output(actions, prefix, "compiler-class-to-source.json", uses_content_based_paths)
        if emit_compiler_class_to_source_map
        else None,
        annotations = declare_prefixed_output(actions, prefix, "__gen__", uses_content_based_paths, dir = True) if declare_annotations else None,
    )

def encode_output_paths(label: Label, paths: OutputPaths, target_type: TargetType) -> struct:
    paths = struct(
        outputJarDirPath = cmd_args(paths.jar.as_output(), parent = 1),
        annotationPath = paths.annotations.as_output() if paths.annotations else None,
        outputJarPath = paths.jar.as_output(),
        compilerClassToSourceMap = paths.compiler_class_to_source_map.as_output() if paths.compiler_class_to_source_map else None,
    )

    return struct(
        libraryPaths = paths if target_type == TargetType("library") else None,
        sourceAbiPaths = paths if target_type == TargetType("source_abi") else None,
        sourceOnlyAbiPaths = paths if target_type == TargetType("source_only_abi") else None,
        libraryTargetFullyQualifiedName = base_qualified_name(label),
    )

def encode_jar_params(remove_classes: list[str], output_paths: OutputPaths, manifest_file: Artifact | None) -> struct:
    return struct(
        jarPath = output_paths.jar.as_output(),
        removeEntryPredicate = struct(
            patterns = remove_classes,
        ),
        manifestFile = manifest_file,
    )

def command_abi_generation_mode(target_type: TargetType, abi_generation_mode: [AbiGenerationMode, None]) -> [AbiGenerationMode, None]:
    # We want the library target to have the real generation mode, but the source one's just use their own.
    # The generation mode will be used elsewhere to setup the target's classpath entry to use the correct abi.
    if target_type == TargetType("source_abi"):
        return AbiGenerationMode("source")
    if target_type == TargetType("source_only_abi"):
        return AbiGenerationMode("source_only")
    return abi_generation_mode

def _source_only_abi_jars(entries: list[JavaClasspathEntry]):
    return [entry.abi for entry in entries]

def _source_only_abi_and_dir(entries: list[JavaClasspathEntry]):
    return [path for entry in entries for path in (entry.abi, entry.abi_as_dir or "")]

def _source_only_abi_compiler_depth(children, entries):
    return max_compiler_depth(children + [entry.abi_compiler_depth for entry in (entries or [])])

SourceOnlyAbiCompilingDepsTSet = transitive_set(
    args_projections = {
        "source_only_abi_and_dir": _source_only_abi_and_dir,
        "source_only_abi_jars": _source_only_abi_jars,
    },
    reductions = {"source_only_abi_compiler_depth": _source_only_abi_compiler_depth},
)

def get_source_only_abi_compiling_deps(
    actions: AnalysisActions,
    compiling_deps_tset: [JavaCompilingDepsTSet, None],
    source_only_abi_deps: list[Dependency],
) -> JavaCompilingDepsTSet | SourceOnlyAbiCompilingDepsTSet:
    if not source_only_abi_deps and compiling_deps_tset != None:
        return compiling_deps_tset
    source_only_abi_compiling_deps = []
    if compiling_deps_tset:
        source_only_abi_deps_filter = {}
        for d in source_only_abi_deps:
            info = d.get(JavaLibraryInfo)
            if not info:
                fail("source_only_abi_deps must produce a JavaLibraryInfo but '{}' does not, please remove it".format(d.label))
            if info.library_output:
                source_only_abi_deps_filter[info.library_output.abi] = True

        def filter_compiling_deps(dep):
            return dep.abi in source_only_abi_deps_filter or dep.required_for_source_only_abi

        source_only_abi_compiling_deps = [compiling_dep for compiling_dep in list(compiling_deps_tset.traverse()) if filter_compiling_deps(compiling_dep)]
    return actions.tset(SourceOnlyAbiCompilingDepsTSet, value = source_only_abi_compiling_deps)

def _apply_ap_param_overrides(params: list[str], overrides: dict[str, [str, None]]) -> list[str]:
    result = [param for param in params if param.split("=", 1)[0].removeprefix("-A") not in overrides]
    for key, value in overrides.items():
        if value != None:
            result.append(key + "=" + value)
    return result

# buildifier: disable=unused-variable
def encode_ap_params(annotation_processor_properties: AnnotationProcessorProperties, target_type: TargetType) -> [struct, None]:
    # buck1 oddly only inspects annotation processors, not plugins for
    # abi/source-only abi related things, even though the plugin rules
    # support the flags. we apply it to both.
    encoded_ap_params = None
    if annotation_processor_properties.annotation_processors:
        parameters = annotation_processor_properties.annotation_processor_params
        if target_type == TargetType("source_only_abi"):
            parameters = _apply_ap_param_overrides(
                parameters,
                annotation_processor_properties.source_only_abi_annotation_processor_param_overrides,
            )
        encoded_ap_params = struct(
            parameters = parameters,
            pluginProperties = [],
        )
        for ap in annotation_processor_properties.annotation_processors:
            # We should also filter out non-abi-affecting APs for source-abi, but buck1 doesn't and so we have lots that depend on not filtering them there.
            if target_type == TargetType("source_only_abi") and not ap.affects_abi:
                continue
            if ap.deps or ap.processors:
                encoded_ap_params.pluginProperties.append(
                    struct(
                        canReuseClassLoader = not ap.isolate_class_loader,
                        doesNotAffectAbi = not ap.affects_abi,
                        supportsAbiGenerationFromSource = ap.supports_source_only_abi,
                        runsOnJavaOnly = ap.runs_on_java_only,
                        processorNames = ap.processors,
                        classpath = ap.deps.project_as_json("javacd_json") if ap.deps else [],
                    ),
                )
    return encoded_ap_params

def encode_plugin_params(plugin_params: [PluginParams, None]) -> [struct, None]:
    encoded_plugin_params = None
    if plugin_params:
        encoded_plugin_params = struct(
            pluginProperties = [encode_plugin_properties(processor, arguments, plugin_params) for processor, arguments in plugin_params.processors],
        )
    return encoded_plugin_params

def encode_plugin_properties(processor: str, arguments: cmd_args, plugin_params: PluginParams) -> struct:
    return struct(
        canReuseClassLoader = False,
        doesNotAffectAbi = False,
        supportsAbiGenerationFromSource = False,
        runsOnJavaOnly = False,
        processorNames = [processor],
        classpath = plugin_params.deps.project_as_json("javacd_json") if plugin_params.deps else [],
        pathParams = {},
        arguments = arguments,
    )

def encode_base_jar_command(
    target_type: TargetType,
    output_paths: OutputPaths,
    remove_classes: list[str],
    label: Label,
    compiling_deps_tset: [JavaCompilingDepsTSet, None],
    classpath_jars_tag: ArtifactTag,
    bootclasspath_entries: list[Artifact],
    system_image: Artifact | None,
    source_level: int,
    target_level: int,
    abi_generation_mode: [AbiGenerationMode, None],
    srcs: list[Artifact],
    resources_map: dict[str, Artifact],
    annotation_processor_properties: AnnotationProcessorProperties,
    plugin_params: [PluginParams, None],
    manifest_file: Artifact | None,
    extra_arguments: cmd_args,
    source_only_abi_compiling_deps: JavaCompilingDepsTSet | SourceOnlyAbiCompilingDepsTSet | None,
    track_class_usage: bool,
    provide_classpath_snapshot: bool = False,
    use_abi_dirs: bool = False,
    incremental_metadata_ignored_inputs_tag: ArtifactTag | None = None,
) -> struct:
    jar_parameters = encode_jar_params(remove_classes, output_paths, manifest_file)
    qualified_name = get_qualified_name(label, target_type)
    if target_type == TargetType("source_only_abi"):
        expect(source_only_abi_compiling_deps != None)
        # A list-valued JSON projection would introduce an extra array level.
        compiling_classpath = cmd_args(source_only_abi_compiling_deps.project_as_args("source_only_abi_and_dir" if use_abi_dirs else "source_only_abi_jars"))
        compiling_classpath_snapshot = []
    else:
        expect(source_only_abi_compiling_deps == None)

        if use_abi_dirs:
            compiling_classpath = cmd_args(compiling_deps_tset.project_as_args("abi_and_dir", ordering = "topological")) if compiling_deps_tset else []
        else:
            compiling_classpath = cmd_args(compiling_deps_tset.project_as_args("args_for_compiling", ordering = "topological")) if compiling_deps_tset else []
        # The snapshot inputs are tagged for association with dep_files, but they are not marked as used,
        # as they serve the incremental compiler's internal needs,
        # which are utilized after the build system has determined whether a rebuild is necessary.
        compiling_classpath_snapshot = classpath_jars_tag.tag_artifacts(
            compiling_deps_tset.project_as_json("abi_snapshot_json", ordering = "topological") if provide_classpath_snapshot and compiling_deps_tset else []
        )

    compiling_classpath = classpath_jars_tag.tag_artifacts(compiling_classpath)
    if use_abi_dirs and incremental_metadata_ignored_inputs_tag:
        compiling_classpath = incremental_metadata_ignored_inputs_tag.tag_artifacts(compiling_classpath)

    build_target_value = struct(
        fullyQualifiedName = qualified_name,
        type = encode_target_type(target_type),
    )
    resolved_java_options = struct(
        bootclasspathList = bootclasspath_entries,
        languageLevelOptions = struct(
            sourceLevel = source_level,
            targetLevel = target_level,
        ),
        javaAnnotationProcessorParams = encode_ap_params(annotation_processor_properties, target_type),
        standardJavacPluginParams = encode_plugin_params(plugin_params),
        extraArguments = extra_arguments,
        systemImage = system_image,
    )

    return struct(
        outputPathsValue = encode_output_paths(label, output_paths, target_type),
        compileTimeClasspathPaths = [] if use_abi_dirs else compiling_classpath,
        compileTimeClasspathAbiAndDirPaths = compiling_classpath if use_abi_dirs else [],
        compileTimeClasspathSnapshotPaths = compiling_classpath_snapshot,
        javaSrcs = srcs,
        abiGenerationMode = encode_abi_generation_mode(command_abi_generation_mode(target_type, abi_generation_mode)),
        trackClassUsage = track_class_usage,
        buildTargetValue = build_target_value,
        resourcesMap = [
            {
                "key": v,
                "value": k,
            }
            for (k, v) in resources_map.items()
        ],
        resolvedJavacOptions = resolved_java_options,
        jarParameters = jar_parameters,
        annotationsPath = output_paths.annotations.as_output() if output_paths.annotations else None,
    )

def setup_dep_files(
    actions: AnalysisActions,
    actions_identifier: [str, None],
    post_build_params: dict,
    classpath_jars_tag: ArtifactTag,
    used_classes_json_outputs: list[cmd_args],
    used_jars_json_output: Artifact | None,
    uses_content_based_paths: bool,
):
    dep_file = declare_prefixed_output(actions, actions_identifier, "jar/dep-file.txt", uses_content_based_paths)

    post_build_params["usedClasses"] = used_classes_json_outputs
    post_build_params["depFile"] = classpath_jars_tag.tag_artifacts(dep_file.as_output())
    if used_jars_json_output != None:
        post_build_params["usedJarsFile"] = used_jars_json_output.as_output()

FORCE_PERSISTENT_WORKERS = read_root_config("build", "require_persistent_workers", "false").lower() == "true"

def prepare_cd_exe(
    qualified_name: str,
    java: RunInfo,
    class_loader_bootstrapper: Artifact,
    compiler: Artifact,
    main_class: str,
    worker: WorkerInfo | None,
    remote_worker: WorkerInfo | None,
    target_specified_debug_port: [int, None],
    toolchain_specified_debug_port: [int, None],
    toolchain_specified_debug_target: [Label, None],
    extra_jvm_args: list[str],
    extra_jvm_args_target: list[Label],
    java_runtime_version: [int, None],
) -> tuple:
    local_only = False
    jvm_args = ["-XX:-MaxFDLimit"] + unsafe_memory_access_jvm_args(java_runtime_version)

    # The variables 'extra_jvm_args' and 'extra_jvm_args_target' are generally used, but they are primarily designed for profiling use-cases.
    # The following section is configured with the profiling use-case in mind.
    if extra_jvm_args_target:
        if len(extra_jvm_args_target) == 1:
            # If there's only one target to profile, we want to isolate its compilation.
            # This target should be built in its own action, allowing the worker (if available) to handle the remaining targets.
            if qualified_name == qualified_name_with_subtarget(extra_jvm_args_target[0]):
                jvm_args = jvm_args + extra_jvm_args
                local_only = True  # This flag ensures the target is not run on the worker.
        else:
            # If there are multiple targets to profile, they should be built on the worker to generate a single profiling data set.
            # The remaining targets should be built individually, either locally or on the Remote Execution (RE).
            local_only = True  # By default, targets are not run on the worker.
            for target in extra_jvm_args_target:
                # If the current target matches the qualified name with subtarget, it is selected for profiling.
                if qualified_name == qualified_name_with_subtarget(target):
                    jvm_args = jvm_args + extra_jvm_args
                    local_only = False  # This flag allows the target to run on the worker.
                    break
    else:
        # If no specific target is provided, the extra JVM arguments are added to all targets that run on worker, local machine or RE.
        jvm_args = jvm_args + extra_jvm_args

    # Allow JVM compiler daemon to access internal jdk.compiler APIs
    jvm_args += [
        "--add-exports=jdk.compiler/com.sun.tools.javac.code=ALL-UNNAMED",
        "--add-exports=jdk.compiler/com.sun.tools.javac.comp=ALL-UNNAMED",
        "--add-exports=jdk.compiler/com.sun.tools.javac.file=ALL-UNNAMED",
        "--add-exports=jdk.compiler/com.sun.tools.javac.jvm=ALL-UNNAMED",
        "--add-exports=jdk.compiler/com.sun.tools.javac.main=ALL-UNNAMED",
        "--add-exports=jdk.compiler/com.sun.tools.javac.model=ALL-UNNAMED",
        "--add-exports=jdk.compiler/com.sun.tools.javac.processing=ALL-UNNAMED",
        "--add-exports=jdk.compiler/com.sun.tools.javac.parser=ALL-UNNAMED",
        "--add-exports=jdk.compiler/com.sun.tools.javac.tree=ALL-UNNAMED",
        "--add-opens=jdk.compiler/com.sun.tools.javac.api=ALL-UNNAMED",
        "--add-opens=jdk.compiler/com.sun.tools.javac.code=ALL-UNNAMED",
        "--add-opens=jdk.compiler/com.sun.tools.javac.jvm=ALL-UNNAMED",
        "--add-opens=jdk.compiler/com.sun.tools.javac.processing=ALL-UNNAMED",
        "--add-opens=jdk.compiler/com.sun.tools.javac.util=ALL-UNNAMED",
    ]

    if target_specified_debug_port:
        debug_port = target_specified_debug_port
    elif toolchain_specified_debug_port and qualified_name == qualified_name_with_subtarget(toolchain_specified_debug_target):
        debug_port = toolchain_specified_debug_port
    else:
        debug_port = None

    if debug_port:
        # Do not use a worker when debugging is enabled
        local_only = True
        jvm_args.extend(["-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address={}".format(debug_port)])

    non_worker_args = cmd_args([java, jvm_args, "-cp", compiler, "-jar", class_loader_bootstrapper, main_class])

    if local_only or not worker:
        # buck doesn't support the case where remote_worker is provided but worker is not.
        if not worker:
            expect(remote_worker == None, "When worker is not provided, remote_worker must be None.")
        return RunInfo(args = non_worker_args), local_only
    else:
        worker_run_info = WorkerRunInfo(
            # Specifies the command to compile using a non-worker process, on RE or if workers are disabled
            # RE workers need exe to run the action, but RE persistent workers need it to be empty
            # we should make this less confusing, but it is how things currently work.
            exe = [] if remote_worker else non_worker_args,
            # Specifies the command to initialize a new worker process.
            # This is used for local execution if `build.use_persistent_workers=True`
            worker = worker,
            remote_worker = remote_worker,
        )
        return worker_run_info, FORCE_PERSISTENT_WORKERS

FinalJarOutput = record(
    final_jar = Artifact,
    # The same as final_jar unless there is a jar_postprocessor.
    preprocessed_jar = Artifact,
)

# If there's additional compiled srcs, we need to merge them in and if the
# caller specified an output artifact we need to make sure the jar is in that
# location.
def prepare_final_jar(
    actions: AnalysisActions,
    actions_identifier: [str, None],
    output: Artifact | None,
    output_paths: OutputPaths,
    additional_compiled_srcs: Artifact | None,
    jar_builder: RunInfo,
    jar_postprocessor: [RunInfo, None],
    jar_postprocessor_runner: RunInfo | None,
    zip_scrubber: RunInfo,
    uses_content_based_paths: bool,
    postprocessor_merged_into_compile_action: bool = False,
) -> FinalJarOutput:
    def make_output(jar: Artifact) -> FinalJarOutput:
        if jar_postprocessor and not postprocessor_merged_into_compile_action:
            expect(jar_postprocessor_runner != None, "Must provide a jar_postprocessor_runner if jar_postprocessor is provided!")
            postprocessed_jar = postprocess_jar(actions, zip_scrubber, jar_postprocessor, jar_postprocessor_runner, jar, actions_identifier)
            return FinalJarOutput(final_jar = postprocessed_jar, preprocessed_jar = jar)
        else:
            return FinalJarOutput(final_jar = jar, preprocessed_jar = jar)

    if not additional_compiled_srcs:
        output_jar = output_paths.jar
        if output:
            actions.copy_file(output.as_output(), output_paths.jar)
            output_jar = output

        return make_output(output_jar)

    merged_jar = output
    if not merged_jar:
        merged_jar = declare_prefixed_output(actions, actions_identifier, "merged.jar", uses_content_based_paths)
    files_to_merge = [output_paths.jar, additional_compiled_srcs]
    files_to_merge_file = actions.write(declare_prefixed_name("files_to_merge.txt", actions_identifier), files_to_merge, has_content_based_path = False)
    actions.run(
        cmd_args(
            [
                jar_builder,
                "--output",
                merged_jar.as_output(),
                "--entries-to-jar",
                files_to_merge_file,
            ],
            hidden = files_to_merge,
        ),
        category = "merge_additional_srcs",
        identifier = actions_identifier,
    )

    return make_output(merged_jar)

def encode_command(
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
    build_mode: BuildMode,
    target_type: TargetType,
    output_paths: OutputPaths,
    classpath_jars_tag: ArtifactTag,
    source_only_abi_compiling_deps: JavaCompilingDepsTSet | SourceOnlyAbiCompilingDepsTSet | None,
    track_class_usage: bool,
    use_abi_dirs: bool = False,
    incremental_metadata_ignored_inputs_tag: ArtifactTag | None = None,
) -> struct:
    base_jar_command = encode_base_jar_command(
        target_type,
        output_paths,
        remove_classes,
        label,
        compiling_deps_tset,
        classpath_jars_tag,
        bootclasspath_entries,
        system_image,
        source_level,
        target_level,
        abi_generation_mode,
        srcs,
        resources_map,
        annotation_processor_properties = annotation_processor_properties,
        plugin_params = plugin_params,
        manifest_file = manifest_file,
        extra_arguments = cmd_args(extra_arguments),
        source_only_abi_compiling_deps = source_only_abi_compiling_deps,
        track_class_usage = track_class_usage,
        provide_classpath_snapshot = provide_classpath_snapshot,
        use_abi_dirs = use_abi_dirs,
        incremental_metadata_ignored_inputs_tag = incremental_metadata_ignored_inputs_tag,
    )

    if kotlin_extra_params:
        return struct(
            buildMode = build_mode,
            baseJarCommand = base_jar_command,
            kotlinExtraParams = kotlin_extra_params,
        )
    else:
        return struct(
            buildMode = build_mode,
            baseJarCommand = base_jar_command,
        )

# buildifier: disable=unused-variable
def generate_abi_jars(
    actions: AnalysisActions,
    actions_identifier: [str, None],
    label: Label,
    abi_generation_mode: [AbiGenerationMode, None],
    additional_compiled_srcs: Artifact | None,
    is_building_android_binary: bool,
    class_abi_generator: Dependency,
    final_jar: Artifact,
    compiling_deps_tset: [JavaCompilingDepsTSet, None],
    source_only_abi_deps: list[Dependency],
    class_abi_jar: Artifact | None,
    class_abi_output_dir: Artifact | None,
    track_class_usage: bool,
    encode_abi_command: typing.Callable,
    define_action: typing.Callable,
    uses_content_based_paths: bool,
    compiler_category: str,
    full_library_compiler_depth: JavaCompilerDepth | None,
    additional_compiler_depth: JavaCompilerDepth | None = None,
    kotlin_extra_params_builder: typing.Callable | None = None,
    source_only_abi_compiling_deps: JavaCompilingDepsTSet | SourceOnlyAbiCompilingDepsTSet | None = None,
) -> tuple:
    class_abi = None
    source_abi = None
    source_only_abi = None
    classpath_abi = None
    classpath_abi_dir = None
    classpath_compiler_depth = full_library_compiler_depth

    # If we are merging additional compiled_srcs, we can't produce source/source-only abis. Otherwise we
    # always generation the source/source-only abis and setup the classpath entry to use the appropriate
    # abi. This allows us to build/inspect/debug source/source-only abi for rules that don't have it enabled.
    if not additional_compiled_srcs:
        if abi_generation_mode == AbiGenerationMode("source") or not is_building_android_binary:
            source_abi_identifier = declare_prefixed_name("source_abi", actions_identifier)
            source_abi_target_type = TargetType("source_abi")
            source_abi_qualified_name = get_qualified_name(label, source_abi_target_type)
            source_abi_output_paths = define_output_paths(actions, source_abi_identifier, label, uses_content_based_paths, declare_annotations = False)
            source_abi_classpath_jars_tag = actions.artifact_tag()
            source_abi_dir = declare_prefixed_output(actions, source_abi_identifier, "source-abi-dir", uses_content_based_paths, dir = True)

            if kotlin_extra_params_builder:
                source_abi_kotlin_extra_params = kotlin_extra_params_builder()
                source_abi_encode_abi_command = encode_abi_command(kotlin_extra_params = source_abi_kotlin_extra_params, provide_classpath_snapshot = False)
            else:
                source_abi_encode_abi_command = encode_abi_command

            source_abi_command = source_abi_encode_abi_command(
                build_mode = BuildMode("ABI"),
                target_type = source_abi_target_type,
                output_paths = source_abi_output_paths,
                classpath_jars_tag = source_abi_classpath_jars_tag,
                source_only_abi_compiling_deps = None,
                track_class_usage = track_class_usage,
            )
            define_action(
                "source_abi_",
                source_abi_identifier,
                source_abi_command,
                source_abi_qualified_name,
                source_abi_output_paths,
                source_abi_classpath_jars_tag,
                source_abi_dir,
                source_abi_target_type,
            )
            source_abi = source_abi_output_paths.jar

            if abi_generation_mode == AbiGenerationMode("source"):
                classpath_abi = source_abi
                classpath_abi_dir = source_abi_dir
                classpath_compiler_depth = compiler_depth_for_action(
                    label,
                    "source_abi_" + compiler_category,
                    compiling_deps_tset,
                    identifier = source_abi_identifier,
                    additional_depth = additional_compiler_depth,
                )

        if abi_generation_mode == AbiGenerationMode("source_only") or not is_building_android_binary:
            source_only_abi_identifier = declare_prefixed_name("source_only_abi", actions_identifier)
            source_only_abi_target_type = TargetType("source_only_abi")
            source_only_abi_qualified_name = get_qualified_name(label, source_only_abi_target_type)
            source_only_abi_output_paths = define_output_paths(actions, source_only_abi_identifier, label, uses_content_based_paths, declare_annotations = False)
            source_only_abi_classpath_jars_tag = actions.artifact_tag()
            source_only_abi_dir = declare_prefixed_output(actions, source_only_abi_identifier, "dir", uses_content_based_paths, dir = True)
            if source_only_abi_compiling_deps == None:
                source_only_abi_compiling_deps = get_source_only_abi_compiling_deps(actions, compiling_deps_tset, source_only_abi_deps)

            if kotlin_extra_params_builder:
                source_only_abi_kotlin_extra_params = kotlin_extra_params_builder()
                source_only_abi_encode_abi_command = encode_abi_command(
                    kotlin_extra_params = source_only_abi_kotlin_extra_params, provide_classpath_snapshot = False
                )
            else:
                source_only_abi_encode_abi_command = encode_abi_command

            source_only_abi_command = source_only_abi_encode_abi_command(
                build_mode = BuildMode("ABI"),
                target_type = source_only_abi_target_type,
                output_paths = source_only_abi_output_paths,
                classpath_jars_tag = source_only_abi_classpath_jars_tag,
                source_only_abi_compiling_deps = source_only_abi_compiling_deps,
                track_class_usage = track_class_usage,
            )
            define_action(
                "source_only_abi_",
                source_only_abi_identifier,
                source_only_abi_command,
                source_only_abi_qualified_name,
                source_only_abi_output_paths,
                source_only_abi_classpath_jars_tag,
                source_only_abi_dir,
                source_only_abi_target_type,
            )
            source_only_abi = source_only_abi_output_paths.jar

            if abi_generation_mode == AbiGenerationMode("source_only"):
                classpath_abi = source_only_abi
                classpath_abi_dir = source_only_abi_dir
                classpath_compiler_depth = compiler_depth_for_action(
                    label,
                    "source_only_abi_" + compiler_category,
                    source_only_abi_compiling_deps,
                    identifier = source_only_abi_identifier,
                    source_only = True,
                    additional_depth = additional_compiler_depth,
                )

        if abi_generation_mode == AbiGenerationMode("none"):
            classpath_abi = final_jar

    if classpath_abi == None or not is_building_android_binary:
        class_abi = class_abi_jar or create_abi(actions, class_abi_generator, final_jar)
        if classpath_abi == None:
            classpath_abi = class_abi
            if class_abi_output_dir:
                classpath_abi_dir = class_abi_output_dir

    return class_abi, source_abi, source_only_abi, classpath_abi, classpath_abi_dir, classpath_compiler_depth

def postprocess_jar(
    actions: AnalysisActions,
    zip_scrubber: RunInfo,
    jar_postprocessor: RunInfo,
    jar_postprocessor_runner: RunInfo,
    original_jar: Artifact,
    actions_identifier: [str, None],
) -> Artifact:
    jar_path = original_jar.short_path
    postprocessed_output = actions.declare_output("postprocessed_{}".format(jar_path), has_content_based_path = True)

    postprocess_jar_cmd = cmd_args(
        jar_postprocessor_runner,
        "--postprocessor_cmd",
        cmd_args(
            [
                jar_postprocessor,
                original_jar,
                postprocessed_output.as_output(),
            ],
            delimiter = " ",
        ),
        "--zip_scrubber",
        cmd_args(zip_scrubber, delimiter = " "),
        "--output",
        postprocessed_output.as_output(),
    )

    identifier = actions_identifier if actions_identifier else ""
    actions.run(postprocess_jar_cmd, category = "postprocessed{}".format(identifier))

    return postprocessed_output
