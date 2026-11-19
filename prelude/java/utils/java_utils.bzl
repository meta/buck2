# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:is_full_meta_repo.bzl", "is_full_meta_repo")
load(
    "@prelude//java:class_to_srcs.bzl",
    "JavaClassToSourceMapInfo",  # @unused Used as a type
    "create_class_to_source_map_from_jar",
    "create_class_to_source_map_info",
    "maybe_create_class_to_source_map_debuginfo",
)
load(
    "@prelude//java:java_providers.bzl",
    "JavaCompileOutputs",  # @unused Used as a type
    "JavaPackagingInfo",  # @unused Used as a type
)
load("@prelude//java:java_toolchain.bzl", "AbiGenerationMode", "JavaToolchainInfo")
load("@prelude//utils:expect.bzl", "expect")

CustomJdkInfo = record(
    # Used with -bootclasspath flag for JDK 8 and older release targets
    # This is also added to the normal classpath when used with kotlinc or javac with JDK9+ release
    bootclasspath = list[Artifact],
    bootclasspath_jar_snapshots = list[Artifact],
    # Used with --system flag for JDK 9+ release targets
    system_image = Artifact,
)

def derive_javac(javac_attribute: [str, Dependency, Artifact]) -> [str, RunInfo, Artifact]:
    javac_attr_type = type(javac_attribute)
    if isinstance(javac_attribute, Dependency):
        javac_run_info = javac_attribute.get(RunInfo)
        if javac_run_info:
            return javac_run_info
        outputs = javac_attribute[DefaultInfo].default_outputs
        expect(len(outputs) == 1, "Expect one default output from build dep of attr javac!")
        return outputs[0]

    if isinstance(javac_attribute, Artifact):
        return javac_attribute

    if javac_attr_type == type(""):
        return javac_attribute

    fail(
        'Type of attribute javac {} that equals to {} is not supported.\n Supported types are "dependency", "artifact" and "string".'.format(
            javac_attr_type, javac_attribute
        )
    )

def get_java_version_attributes(ctx: AnalysisContext) -> (int, int):
    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]

    # used for multi-release jars
    # the attribute might not exist if function is being used in other macros, such as kotlin_library
    min_release_version = ctx.attrs.min_release_version if hasattr(ctx.attrs, "min_release_version") else None
    java_version = ctx.attrs.java_version
    java_source = ctx.attrs.source
    java_target = ctx.attrs.target

    if java_version or min_release_version:
        if java_source or java_target:
            fail("No need to set 'source' and/or 'target' attributes when 'java_version' or 'min_release_version' are present")
        java_version = to_java_version(min_release_version) if min_release_version else to_java_version(java_version)
        return (java_version, java_version)

    source = java_source or java_toolchain.source_level
    target = java_target or java_toolchain.target_level

    expect(bool(source) and bool(target), "Java source level and target level must be set!")

    source = to_java_version(source)
    target = to_java_version(target)

    expect(source <= target, "java library source level {} is higher than target {} ", source, target)

    return (source, target)

def to_java_version(java_version: str) -> int:
    if java_version.startswith("1."):
        expect(len(java_version) == 3, "Supported java version number format is 1.X, where X is a single digit number, but it was set to {}", java_version)
        java_version_number = int(java_version[2:])
        expect(
            java_version_number < 9,
            "Supported java version number format is 1.X, where X is a single digit number that is less than 9, but it was set to {}",
            java_version,
        )
        return java_version_number
    else:
        return int(java_version)

def get_string_concat_inline_javac_args(source_level: int) -> list[str]:
    # Android build tools before SDK 36 lack java.lang.invoke.StringConcatFactory, which javac
    # targets for invokedynamic string concatenation at -source 9+ (JEP 280). Force javac to emit
    # classic StringBuilder concatenation instead so the bytecode compiles and runs against those SDKs.
    if source_level >= 9:
        return ["-XDstringConcat=inline"]
    return []

def get_abi_generation_mode(abi_generation_mode):
    return {
        None: None,
        "class": AbiGenerationMode("class"),
        "migrating_to_source_only": AbiGenerationMode("source"),
        "none": AbiGenerationMode("none"),
        "source": AbiGenerationMode("source"),
        "source_only": AbiGenerationMode("source_only"),
    }[abi_generation_mode]

def get_default_info(
    actions: AnalysisActions,
    java_toolchain: JavaToolchainInfo,
    outputs: [JavaCompileOutputs, None],
    packaging_info: JavaPackagingInfo,
    extra_sub_targets: dict = {},
) -> DefaultInfo:
    sub_targets = get_classpath_subtargets(actions, packaging_info)
    default_info = DefaultInfo(
        sub_targets = extra_sub_targets | sub_targets,
    )
    if outputs:
        abis = [
            ("class-abi", outputs.class_abi),
            ("source-abi", outputs.source_abi),
            ("source-only-abi", outputs.source_only_abi),
        ]
        for name, artifact in abis:
            if artifact != None:
                sub_targets[name] = [DefaultInfo(default_output = artifact)]
        if outputs.command_proto_json != None:
            sub_targets["jar_command_proto_json"] = [DefaultInfo(default_output = outputs.command_proto_json)]
        other_outputs = []
        if not java_toolchain.is_bootstrap_toolchain and outputs.annotation_processor_output:
            other_outputs.append(outputs.annotation_processor_output)
        default_info = DefaultInfo(
            default_output = outputs.full_library,
            sub_targets = extra_sub_targets | sub_targets,
            other_outputs = other_outputs,
        )
    return default_info

def declare_prefixed_name(name: str, prefix: [str, None]) -> str:
    if not prefix:
        return name

    return "{}_{}".format(prefix, name)

def get_class_to_source_map_info(
    ctx: AnalysisContext,
    outputs: [JavaCompileOutputs, None],
    deps: list[Dependency],
    generate_sources_jar: bool = False,
    class_to_src_map_deps: list[Dependency] = [],
) -> (JavaClassToSourceMapInfo, Artifact | None, dict):
    sub_targets = {}
    class_to_srcs = None
    mapping_debuginfo = None
    sources_jar = None
    if outputs != None:
        name = ctx.label.name
        java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
        mapping_debuginfo = maybe_create_class_to_source_map_debuginfo(
            actions = ctx.actions,
            java_toolchain = java_toolchain,
            name = name + ".debuginfo.json",
            srcs = ctx.attrs.srcs,
        )
        class_to_srcs, sources_jar = create_class_to_source_map_from_jar(
            actions = ctx.actions,
            java_toolchain = java_toolchain,
            name = name + ".class_to_srcs.json",
            jar = outputs.classpath_entry.full_library,
            srcs = ctx.attrs.srcs,
            owner_target = str(ctx.label.raw_target()),
            sources_jar_name = "{}-sources.jar".format(name) if generate_sources_jar else None,
            debuginfo = mapping_debuginfo,
            compiler_map = outputs.compiler_class_to_source_map,
        )
        sub_targets["class-to-srcs"] = [DefaultInfo(default_output = class_to_srcs)]
        if outputs.compiler_class_to_source_map != None:
            sub_targets["compiler-class-to-srcs"] = [DefaultInfo(default_output = outputs.compiler_class_to_source_map)]
        if sources_jar:
            sub_targets["sources.jar"] = [DefaultInfo(default_output = sources_jar)]

    # Include class_to_src_map_deps for classmap collection. These are deps that
    # only contribute to the class-to-source map (for debugging) but not to compilation.
    all_classmap_deps = deps + class_to_src_map_deps
    class_to_src_map_info = create_class_to_source_map_info(
        ctx = ctx,
        mapping = class_to_srcs,
        mapping_debuginfo = mapping_debuginfo,
        deps = all_classmap_deps,
    )
    if outputs != None:
        sub_targets["debuginfo"] = [DefaultInfo(default_output = class_to_src_map_info.debuginfo)]
    return (class_to_src_map_info, sources_jar, sub_targets)

def get_classpath_subtargets(actions: AnalysisActions, packaging_info: JavaPackagingInfo) -> dict[str, list[Provider]]:
    proj = packaging_info.packaging_deps.project_as_args("full_jar_args")
    output = actions.write("classpath", proj, has_content_based_path = False)

    classpath_targets_proj = packaging_info.packaging_deps.project_as_args("full_jar_owner_args")
    classpath_targets_output = actions.write("classpath_targets", classpath_targets_proj, has_content_based_path = False)

    return {
        "classpath": [DefaultInfo(output, other_outputs = [proj])],
        "classpath_targets": [DefaultInfo(classpath_targets_output)],
    }

def build_bootclasspath(bootclasspath_entries: list[Artifact], source_level: int, java_toolchain: JavaToolchainInfo) -> list[Artifact]:
    bootclasspath_list = []

    if bootclasspath_entries:
        bootclasspath_list = bootclasspath_entries
    elif source_level == 8:
        if not is_full_meta_repo():
            return bootclasspath_list
        expect(java_toolchain.bootclasspath_8, "Must specify bootclasspath for source level 8")
        bootclasspath_list = java_toolchain.bootclasspath_8
    return bootclasspath_list
