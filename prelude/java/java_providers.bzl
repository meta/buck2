# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:resources.bzl",
    "ResourceInfo",  # @unused Used as a type
    "gather_resources",
    "make_resource_info",
)
load("@prelude//java:class_to_srcs.bzl", "JavaClassToSourceMapInfo")
load("@prelude//java:compiler_depth.bzl", "JavaCompilerDepth", "max_compiler_depth")
load("@prelude//java:dex.bzl", "DexLibraryInfo", "get_dex_produced_from_java_library")
load("@prelude//java:dex_toolchain.bzl", "DexToolchainInfo")
load("@prelude//java/utils:java_more_utils.bzl", "get_path_separator_for_exec_os")
load("@prelude//linking:linkable_graph.bzl", "LinkableGraph", "create_linkable_graph")
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
    "merge_shared_libraries",
)
load("@prelude//utils:expect.bzl", "expect")

# JAVA PROVIDER DOCS
#
# Our core Java provider is JavaLibraryInfo. At a basic level, this provider needs to give
# its dependents the ability to do two things: compilation and packaging.
#
# Compilation
#
# When we compile, we need to add all of our dependencies to the classpath. That includes
# anything in `deps`, `exported_deps`, `provided_deps` and `exported_provided_deps`, (but
# not `runtime_deps`). Additionally, it includes anything that these dependencies export
# (via `exported_deps` or `exported_provided_deps`). For example, if A depends upon B,
# and B has an exported dependency on C, then we need to add both B and C to the classpath
# when compiling A, i.e. both B and C need to be part of B's `compiling_deps`.
#
# Therefore, the `compiling_deps` consist of the library's own output (if it exists) plus
# the `compiling_deps` of any `exported_deps` and `exported_provided_deps`.
#
# When we compile, we don't need to compile against the full library - instead, we just
# compile against the library's public interface, or ABI.
#
# Packaging
#
# When we package our Java code into a `java_binary`, we need to include all of the Java
# code that is need to run the application - i.e. all the transitive dependencies. That
# includes anything in `deps`, `exported_deps` and `runtime_deps` (but not `provided_deps`
# or `exported_provided_deps`). For example, if A depends upon B, and B has a `dep` on C
# and a `provided_dep` on D, then if we package A we also need to include B and C, but
# not D.
#
# Therefore, the `packaging_deps` consist of the library's own output (if it exists) plus
# the `packaging_deps` of any `deps`, `exported_deps` and `runtime_deps`.
#
# When we package, we need to use the full library (since we are actually going to be
# running the code contained in the library).
#
# We also need to package up any native code that is declared transitively. The
# `SharedLibraryInfo` also consists of the `SharedLibraryInfo` of any `deps`,
# `exported_deps` and `runtime_deps`.
#
# Android
#
# Because Android uses Java, and we don't currently have the ability to "overlay" our
# providers, the core Java providers are extended to support Android's requirements.
# This introduces some additional complexity.
#
# Android doesn't package Java bytecode, but instead it converts the Java bytecode
# .dex (Dalvik Executable) files that are packaged into the Android binary (either an
# APK or an AAB). Therefore, our `packaging_deps` contain not just a `jar` field but
# also a `dex` field. If the `dex` field is empty, then the dep should not be
# packaged into the APK - this is useful for things like `android_build_config` where
# we want the output (.jar) to be present in any Java annotation processors that we
# run, but not in the final binary (since we rewrite the build config at the binary
# level anyway).
#
# Android also provides the ability to run Proguard on your binary in order to
# remove unused classes etc. Each Java library can specify any classes that it wants
# to always keep etc, via a `proguard_config`. This config also needs to be added to
# the `packaging_deps`.
#
# Java-like rules also provide a "special" function that can be used inside queries:
# "classpath". `classpath(A)` returns all of the packaging deps of A, while
# `classpath(A, 1)` returns all of the first-order packaging deps of A.

JavaClasspathEntry = record(
    full_library = field(Artifact),
    abi = field(Artifact),
    # abi_as_dir is the abi .jar unzipped into a directory. If available, it is used to provide
    # .class level granularity for javacd and kotlincd dep-files.
    abi_as_dir = field(Artifact | None),
    required_for_source_only_abi = field(bool),
    abi_jar_snapshot = field(Artifact | None),
    # Source and source-only ABIs can be produced independently of the full library.
    full_library_compiler_depth = field(JavaCompilerDepth | None, None),
    abi_compiler_depth = field(JavaCompilerDepth | None, None),
)

def _args_for_ast_dumper(entry: JavaClasspathEntry):
    return [
        "--dependency",
        '"{}"'.format(entry.abi.owner),
        entry.abi,
    ]

def _args_for_compiling(entry: JavaClasspathEntry):
    return entry.abi

def _abi_snapshot_json(entry: JavaClasspathEntry):
    if entry.abi and entry.abi_jar_snapshot:
        return cmd_args([entry.abi_jar_snapshot], delimiter = " ")

    return None

def _abi_dirs(entry: JavaClasspathEntry):
    return entry.abi_as_dir or []

def _abi_and_dir(entry: JavaClasspathEntry):
    return [entry.abi, entry.abi_as_dir or ""]

def _source_only_abi_jars(entry: JavaClasspathEntry):
    return [entry.abi] if entry.required_for_source_only_abi else []

def _source_only_abi_and_dir(entry: JavaClasspathEntry):
    return _abi_and_dir(entry) if entry.required_for_source_only_abi else []

def _full_library_args(entry: JavaClasspathEntry):
    return entry.full_library

# This doesn't have any projections on it at all. It exists only for the
# purpose of propagating JavaCompilingDepsTSet nodes up the graph in
# JavaGlobalCodeInfo.
#
# We then traverse this wrapper to get the underlying JavaCompilingDepsTSets.
# It is much, much cheaper in memory usage to do this than it is to ensure
# a TSet projection at every node for every different global code info spec.
JavaCompilingDepsTSetWrapper = transitive_set()

def _compiler_depth(children, entry):
    return max_compiler_depth(children + [entry.abi_compiler_depth if entry else None])

def _source_only_abi_compiler_depth(children, entry):
    return max_compiler_depth(children + [entry.abi_compiler_depth if entry and entry.required_for_source_only_abi else None])

JavaCompilingDepsTSet = transitive_set(
    args_projections = {
        "abi_and_dir": _abi_and_dir,
        "abi_dirs": _abi_dirs,
        "args_for_ast_dumper": _args_for_ast_dumper,
        "args_for_compiling": _args_for_compiling,
        "full_library_args": _full_library_args,
        "source_only_abi_and_dir": _source_only_abi_and_dir,
        "source_only_abi_jars": _source_only_abi_jars,
    },
    json_projections = {
        "abi_snapshot_json": _abi_snapshot_json,
    },
    reductions = {
        "compiler_depth": _compiler_depth,
        "source_only_abi_compiler_depth": _source_only_abi_compiler_depth,
    },
)

JavaPackagingDep = record(
    label = Label,
    jar = Artifact | None,
    dex = [DexLibraryInfo, None],
    gwt_module = Artifact | None,
    is_prebuilt_jar = bool,
    proguard_config = Artifact | None,
    # An output that is used solely by the system to have an artifact bound to the target (that the core can then use to find
    # the right target from the given artifact).
    output_for_classpath_macro = Artifact,
    sources_jar = Artifact | None,
    compiler_depth = field(JavaCompilerDepth | None, None),
)

def _full_jar_args(dep: JavaPackagingDep):
    if dep.jar:
        return cmd_args(dep.jar)
    return cmd_args()

def _full_jar_owner_args(dep: JavaPackagingDep):
    if dep.jar:
        return cmd_args(dep.label.raw_target())
    return cmd_args()

def _args_for_classpath_macro(dep: JavaPackagingDep):
    return dep.output_for_classpath_macro

def _packaging_dep_javacd_json(dep: JavaPackagingDep):
    return dep.jar or ""

JavaPackagingDepTSet = transitive_set(
    args_projections = {
        "args_for_classpath_macro": _args_for_classpath_macro,
        "full_jar_args": _full_jar_args,
        "full_jar_owner_args": _full_jar_owner_args,
    },
    json_projections = {
        "javacd_json": _packaging_dep_javacd_json,
    },
)

JavaGlobalCodeInfo = provider(
    doc = """This dictionary maps a framework key to its corresponding GlobalCodeConfig. The GlobalCodeConfig specifies the dependency .jars required by the framework for global-level code generation (binary level).
    The process responsible for generating the global_code_info provider for the target utilizes this mapping to:
    * Retrieve the GlobalCodeConfig associated with each framework, identified by its key.
    * Determine whether the .jar files for the library or any of its dependencies are necessary for global code generation for that particular framework.
    * Create a mapping from each framework key to a list of the required .jars identified in the previous step.""",
    fields = {
        "global_code_map": provider_field(typing.Any, default = None),  # "{name: JavaCompilingDepsTSet}"
    },
)

JavaLibraryInfo = provider(
    doc = "Information about a java library and its dependencies",
    fields = {
        # Java dependencies exposed to dependent targets and supposed to be used during compilation.
        # Consisting of this library's own output, and the "compiling_deps" of any exported_deps and exported_provided_deps.
        #
        "compiling_deps": provider_field(typing.Any, default = None),  # ["JavaCompilingDepsTSet", None]
        # A TSet that contains the "compiling_deps" as a node. This is used to propagate "global code info" up the graph.
        "compiling_deps_wrapper": provider_field(typing.Any, default = None),  # ["JavaCompilingDepsTSetWrapper", None]
        # An output of the library. If present then already included into `compiling_deps` field.
        "library_output": provider_field(typing.Any, default = None),  # ["JavaClasspathEntry", None]
        # Shows if the library can be exported or not
        "may_not_be_exported": provider_field(typing.Any, default = None),
        # Shows if the library can be packaged or not
        "may_not_be_packaged": provider_field(typing.Any, default = None),
        # An output that is used solely by the system to have an artifact bound to the target (that the core can then use to find
        # the right target from the given artifact).
        "output_for_classpath_macro": provider_field(typing.Any, default = None),  # "artifact"
    },
)

JavaLibraryIntellijInfo = provider(
    doc = "Information about a java library that is required for Intellij project generation",
    fields = {
        # Directory containing external annotation jars
        "annotation_jars_dir": provider_field(typing.Any, default = None),  # ["artifact", None]
        # All the artifacts that were used in order to compile this library
        "compiling_classpath": provider_field(typing.Any, default = None),  # ["artifact"]
        "generated_sources": provider_field(typing.Any, default = None),  # ["artifact"]
        "lint_jar": provider_field(typing.Any, default = None),  # ["artifact"]
        # If this library has a jar_postprocessor, this is the jar prior to post-processing.
        # Otherwise, it is the same as library_output in JavaLibraryInfo.
        "preprocessed_library": provider_field(typing.Any, default = None),  # ["artifact", None]
        "used_jars_json": provider_field(typing.Any, default = None),  # ["artifact"]
    },
)

JavaPackagingInfo = provider(
    fields = {
        # Presents all java dependencies used to build this library and it's dependencies (all transitive deps except provided ones).
        # These deps must be included into the final artifact.
        "packaging_deps": provider_field(typing.Any, default = None),  # ["JavaPackagingDepTSet", None],
    },
)

JavaCompileOutputs = record(
    full_library = Artifact,
    class_abi = Artifact | None,
    source_abi = Artifact | None,
    source_only_abi = Artifact | None,
    classpath_entry = JavaClasspathEntry,
    annotation_processor_output = Artifact | None,
    preprocessed_library = Artifact,
    incremental_state_dir = Artifact | None,
    used_jars_json = Artifact | None,
    compiler_class_to_source_map = field(Artifact | None, default = None),
)

JavaProviders = record(
    java_library_info = JavaLibraryInfo,
    java_library_intellij_info = JavaLibraryIntellijInfo,
    java_packaging_info = JavaPackagingInfo,
    java_global_code_info = JavaGlobalCodeInfo,
    shared_library_info = SharedLibraryInfo,
    cxx_resource_info = ResourceInfo,
    linkable_graph = LinkableGraph,
    template_placeholder_info = TemplatePlaceholderInfo,
    default_info = DefaultInfo,
    class_to_src_map = [JavaClassToSourceMapInfo, None],
    validation_info = [ValidationInfo, None],
)

def to_list(java_providers: JavaProviders) -> list[Provider]:
    providers = [
        java_providers.java_library_info,
        java_providers.java_library_intellij_info,
        java_providers.java_packaging_info,
        java_providers.java_global_code_info,
        java_providers.shared_library_info,
        java_providers.cxx_resource_info,
        java_providers.linkable_graph,
        java_providers.template_placeholder_info,
        java_providers.default_info,
    ]
    if java_providers.class_to_src_map != None:
        providers.append(java_providers.class_to_src_map)

    if java_providers.validation_info != None:
        providers.append(java_providers.validation_info)

    return providers

# Creates a JavaCompileOutputs. `classpath_abi` can be set to specify a
# specific artifact to be used as the abi for the JavaClasspathEntry.
def make_compile_outputs(
    full_library: Artifact,
    preprocessed_library: Artifact,
    class_abi: Artifact | None = None,
    source_abi: Artifact | None = None,
    source_only_abi: Artifact | None = None,
    classpath_abi: Artifact | None = None,
    classpath_abi_dir: Artifact | None = None,
    required_for_source_only_abi: bool = False,
    annotation_processor_output: Artifact | None = None,
    incremental_state_dir: Artifact | None = None,
    abi_jar_snapshot: Artifact | None = None,
    used_jars_json: Artifact | None = None,
    compiler_class_to_source_map: Artifact | None = None,
    full_library_compiler_depth: JavaCompilerDepth | None = None,
    abi_compiler_depth: JavaCompilerDepth | None = None,
) -> JavaCompileOutputs:
    expect(classpath_abi != None or classpath_abi_dir == None, "A classpath_abi_dir should only be provided if a classpath_abi is provided!")
    return JavaCompileOutputs(
        full_library = full_library,
        class_abi = class_abi,
        source_abi = source_abi,
        source_only_abi = source_only_abi,
        classpath_entry = JavaClasspathEntry(
            full_library = full_library,
            abi = classpath_abi or class_abi or full_library,
            abi_as_dir = classpath_abi_dir,
            required_for_source_only_abi = required_for_source_only_abi,
            abi_jar_snapshot = abi_jar_snapshot,
            full_library_compiler_depth = full_library_compiler_depth,
            abi_compiler_depth = abi_compiler_depth,
        ),
        annotation_processor_output = annotation_processor_output,
        preprocessed_library = preprocessed_library,
        incremental_state_dir = incremental_state_dir,
        used_jars_json = used_jars_json,
        compiler_class_to_source_map = compiler_class_to_source_map,
    )

def create_abi(actions: AnalysisActions, class_abi_generator: Dependency, library: Artifact, keepSynthetic: bool = False) -> Artifact:
    # It's possible for the library to be created in a subdir that is
    # itself some actions output artifact, so we replace directory
    # separators to get a path that we can uniquely own.
    # TODO(cjhopman): This probably should take in the output path.
    class_abi = actions.declare_output("{}-class-abi.jar".format(library.short_path.replace("/", "_")), has_content_based_path = True)
    cmd = [
        class_abi_generator[RunInfo],
        library,
        class_abi.as_output(),
    ]
    if keepSynthetic:
        cmd.append("--keep-synthetic")
    actions.run(
        cmd,
        category = "class_abi_generation",
        identifier = library.short_path,
    )
    return class_abi

ClasspathSnapshotGranularity = enum("CLASS_LEVEL", "CLASS_MEMBER_LEVEL")

def generate_java_classpath_snapshot(
    actions: AnalysisActions, snapshot_generator: Dependency | None, granularity: ClasspathSnapshotGranularity, library: Artifact, action_identifier: str | None
) -> Artifact | None:
    if not snapshot_generator:
        return None
    identifier = ("{}_".format(action_identifier) if action_identifier else "") + library.short_path.replace("/", "_").rsplit(".", 1)[
        0
    ]  # remove the extension and preserve any [foo.baz].[jar] or [thing-1.2.3.4].[jar] style names
    output = actions.declare_output(
        "{}_jar_snapshot_{}.bin".format(
            identifier,
            "cl" if ClasspathSnapshotGranularity("CLASS_LEVEL") == granularity else "cml",
        ),
        has_content_based_path = True,
    )
    actions.run(
        [
            snapshot_generator[RunInfo],
            "--input-jar",
            library,
            "--output-snapshot",
            output.as_output(),
            "--granularity",
            granularity.value,
        ],
        category = "jar_snapshot",
        identifier = identifier,
    )
    return output

def single_library_compiling_deps(actions: AnalysisActions, library_output: [JavaClasspathEntry, None]) -> [JavaCompilingDepsTSet, None]:
    return get_compiling_deps_tset(actions, value = library_output)

def get_compiling_deps_tset(
    actions: AnalysisActions,
    deps: list[Dependency] = [],
    additional_classpath_entries: list[JavaCompilingDepsTSet] = [],
    value: JavaClasspathEntry | None = None,
) -> JavaCompilingDepsTSet | None:
    """Create a classpath root with an optional value before its children."""
    children = [info.compiling_deps for info in filter(None, [dep.get(JavaLibraryInfo) for dep in deps]) if info.compiling_deps != None]
    children += additional_classpath_entries
    if value != None:
        return actions.tset(JavaCompilingDepsTSet, value = value, children = children)
    if not children:
        return None
    if len(children) == 1:
        return children[0]
    return actions.tset(JavaCompilingDepsTSet, children = children)

def single_library_compiling_deps_wrapper(actions: AnalysisActions, compiling_deps_tset: [JavaCompilingDepsTSet, None]) -> [JavaCompilingDepsTSetWrapper, None]:
    if compiling_deps_tset:
        return actions.tset(JavaCompilingDepsTSetWrapper, value = compiling_deps_tset)
    else:
        return None

# Accumulate the wrappers required for propagating JavaCompilingDepsTSets up the graph.
def derive_compiling_deps_wrapper(
    actions: AnalysisActions, library_output: [JavaCompilingDepsTSetWrapper, None], children: list[Dependency]
) -> [JavaCompilingDepsTSetWrapper, None]:
    if children:
        filtered_children = filter(
            None,
            [exported_dep.compiling_deps_wrapper for exported_dep in filter(None, [x.get(JavaLibraryInfo) for x in children])],
        )
        children = filtered_children

    if not library_output and not children:
        return None

    return actions.tset(JavaCompilingDepsTSetWrapper, children = (children or []) + ([library_output] if library_output else []))

def _is_dexing_enabled(ctx: AnalysisContext) -> bool:
    dex_toolchain = getattr(ctx.attrs, "_dex_toolchain", None)
    return dex_toolchain != None and dex_toolchain[DexToolchainInfo].d8_command != None

def create_java_packaging_dep(
    ctx: AnalysisContext,
    library_jar: Artifact | None = None,
    output_for_classpath_macro: Artifact | None = None,
    # Defaults off because callers outside `create_java_library_providers` dex synthetic jars that
    # hold only part of a nest (per-type R.java jars, canary classes), which D8 rejects when nest
    # desugaring is on. Such jars have no cross-nest private access, so skipping is safe.
    needs_desugar: bool = False,
    desugar_deps: [TransitiveSetArgsProjection, None] = None,
    is_prebuilt_jar: bool = False,
    has_srcs: bool = True,
    sources_jar: Artifact | None = None,
    dex_weight_factor: int = 1,
    proguard_config: Artifact | None = None,
    gwt_module: Artifact | None = None,
    compiler_depth: JavaCompilerDepth | None = None,
) -> JavaPackagingDep:
    if library_jar != None and has_srcs and _is_dexing_enabled(ctx):
        dex = get_dex_produced_from_java_library(
            ctx,
            ctx.attrs._dex_toolchain[DexToolchainInfo],
            library_jar,
            needs_desugar,
            desugar_deps,
            dex_weight_factor,
        )
    else:
        dex = None

    expect(library_jar != None or output_for_classpath_macro != None, "Must provide an output_for_classpath_macro if no library_jar is provided!")

    return JavaPackagingDep(
        label = ctx.label,
        jar = library_jar,
        dex = dex,
        gwt_module = gwt_module,
        is_prebuilt_jar = is_prebuilt_jar,
        proguard_config = proguard_config or getattr(ctx.attrs, "proguard_config", None),
        output_for_classpath_macro = output_for_classpath_macro or library_jar,
        sources_jar = sources_jar,
        compiler_depth = compiler_depth,
    )

def get_all_java_packaging_deps(ctx: AnalysisContext, deps: list[Dependency]) -> list[JavaPackagingDep]:
    return get_all_java_packaging_deps_from_packaging_infos(ctx, filter(None, [x.get(JavaPackagingInfo) for x in deps]))

def get_all_java_packaging_deps_from_packaging_infos(ctx: AnalysisContext, infos: list[JavaPackagingInfo]) -> list[JavaPackagingDep]:
    children = filter(None, [info.packaging_deps for info in infos])
    if not children:
        return []

    tset = ctx.actions.tset(JavaPackagingDepTSet, children = children)

    return list(tset.traverse())

def get_all_java_packaging_deps_tset(
    ctx: AnalysisContext, java_packaging_infos: list[JavaPackagingInfo], java_packaging_dep: [JavaPackagingDep, None] = None
) -> [JavaPackagingDepTSet, None]:
    packaging_deps_kwargs = {}
    if java_packaging_dep != None:
        packaging_deps_kwargs["value"] = java_packaging_dep

    packaging_deps_children = filter(None, [info.packaging_deps for info in java_packaging_infos])
    if packaging_deps_children:
        packaging_deps_kwargs["children"] = packaging_deps_children

    return ctx.actions.tset(JavaPackagingDepTSet, **packaging_deps_kwargs) if packaging_deps_kwargs else None

_EMPTY_JAVA_PACKAGING_INFO = JavaPackagingInfo(packaging_deps = None)

# Accumulate deps necessary for packaging, which consist of all transitive java deps (except provided ones)
def get_java_packaging_info(ctx: AnalysisContext, raw_deps: list[Dependency], java_packaging_dep: [JavaPackagingDep, None] = None) -> JavaPackagingInfo:
    java_packaging_infos = filter(None, [x.get(JavaPackagingInfo) for x in raw_deps])
    packaging_deps = get_all_java_packaging_deps_tset(ctx, java_packaging_infos, java_packaging_dep)
    if packaging_deps == None:
        return _EMPTY_JAVA_PACKAGING_INFO
    return JavaPackagingInfo(packaging_deps = packaging_deps)

def _group_global_code_children_by_name(global_code_infos: list[JavaGlobalCodeInfo]) -> dict[str, list[JavaCompilingDepsTSetWrapper]]:
    # Collect the per-framework children from all global_code_infos in a single pass over the infos.
    # Callers build a tset for many frameworks, so grouping once here avoids re-scanning every info
    # for every framework, which would be O(frameworks * deps).
    children_by_name = {}
    for info in global_code_infos:
        for name, child in info.global_code_map.items():
            children_by_name.setdefault(name, []).append(child)

    return children_by_name

def _create_global_code_tset(actions: AnalysisActions, children: list[JavaCompilingDepsTSetWrapper]) -> [JavaCompilingDepsTSetWrapper, None]:
    if not children:
        return None
    elif len(children) == 1:
        return children[0]
    else:
        return actions.tset(JavaCompilingDepsTSetWrapper, children = children)

# This function identifies and collects necessary dependencies that meet criteria defined in `GLOBAL_CODE_CONFIG` for global code generation across frameworks.
# It maps framework names to their corresponding Java compiling dependency sets.
# Example: Below configuration specifies criteria for the "di" framework:
# GLOBAL_CODE_CONFIG = {
#     "di": (
#         triggers = ["//fbandroid/java/com/facebook/inject:inject"],
#         deps = [],
#         requires_first_order_classpath = False,
#     ),
# }
# With this setup, if a target depends on "//fbandroid/java/com/facebook/inject:inject", the `global_code_info` provider for that target will have an entry under "di".
# This entry will be a JavaCompilingDepsTSet containing the .jar files associated with that target.
# Each framework (like "di") can use a Buck rule to identify dependencies with matching values for their framework key in the `global_code_info` provider.
# They can then compile all the .jars needed for global code generation.

def get_global_code_info(
    ctx: AnalysisContext,
    declared_deps: list[Dependency],
    packaging_deps: list[Dependency],
    single_library_dep: [JavaCompilingDepsTSetWrapper, None],
    library_compiling_deps: [JavaCompilingDepsTSetWrapper, None],
    first_order_compiling_deps_without_library_itself: list[JavaCompilingDepsTSetWrapper],
    global_code_config: dict,
) -> JavaGlobalCodeInfo:
    global_code_infos = filter(None, [x.get(JavaGlobalCodeInfo) for x in packaging_deps])
    children_by_name = _group_global_code_children_by_name(global_code_infos)

    declared_deps_raw_targets = set([declared_dep.label.raw_target() for declared_dep in declared_deps])
    raw_target = ctx.label.raw_target()

    def declared_deps_contains_trigger(deps_triggers: set[TargetLabel]) -> TargetLabel | None:
        for trigger in deps_triggers:
            if trigger in declared_deps_raw_targets:
                return trigger

        return None

    global_code_map = {}
    for name, (config) in global_code_config.items():
        target_is_global_code_dep = raw_target in config.deps
        contains_trigger = None
        if declared_deps_raw_targets and not target_is_global_code_dep:
            contains_trigger = declared_deps_contains_trigger(config.triggers)
        if (contains_trigger or target_is_global_code_dep) and config.requires_first_order_classpath:
            global_code_library_compiling_deps = []
            if single_library_dep:
                global_code_library_compiling_deps.append(single_library_dep)
            if first_order_compiling_deps_without_library_itself:
                global_code_library_compiling_deps.extend(first_order_compiling_deps_without_library_itself)
        elif target_is_global_code_dep:
            global_code_library_compiling_deps = [library_compiling_deps]
        elif contains_trigger:
            if single_library_dep == None:
                # We have to have single_library_dep - otherwise there is no output.
                # This /should/ be a failure - however there are WAY too many places that do not follow this pattern to effectively fail here.
                # fail("Target {} contains dep {} which is a 'trigger' for the global code rule {}, but the target does not produce any output. ".format(
                #         ctx.label.raw_target(),
                #         contains_trigger,
                #         name,
                #     ) +
                #     "If the target does not export anything, it can be removed completely, or otherwise just remove all of the deps.")
                global_code_library_compiling_deps = []
            else:
                global_code_library_compiling_deps = [single_library_dep]
        else:
            global_code_library_compiling_deps = []

        children = list(children_by_name.get(name, []))
        if global_code_library_compiling_deps:
            children.extend(global_code_library_compiling_deps)

        global_code_tset = _create_global_code_tset(ctx.actions, children)
        if global_code_tset:
            global_code_map[name] = global_code_tset

    return JavaGlobalCodeInfo(global_code_map = global_code_map)

def propagate_global_code_info(ctx: AnalysisContext, packaging_deps: list[Dependency]) -> JavaGlobalCodeInfo:
    global_code_map = {}
    global_code_infos = filter(None, [x.get(JavaGlobalCodeInfo) for x in packaging_deps])
    children_by_name = _group_global_code_children_by_name(global_code_infos)

    for key, children in children_by_name.items():
        global_code_tset = _create_global_code_tset(ctx.actions, children)
        if global_code_tset:
            global_code_map[key] = global_code_tset

    return JavaGlobalCodeInfo(global_code_map = global_code_map)

def create_native_providers(ctx: AnalysisContext, label: Label, packaging_deps: list[Dependency]) -> (SharedLibraryInfo, ResourceInfo, LinkableGraph):
    shared_library_info = merge_shared_libraries(
        ctx.actions,
        deps = filter(None, [x.get(SharedLibraryInfo) for x in packaging_deps]),
    )
    cxx_resource_info = make_resource_info(
        gather_resources(
            label,
            deps = packaging_deps,
        )
    )

    linkable_graph = create_linkable_graph(ctx, deps = filter(None, [x.get(LinkableGraph) for x in packaging_deps]))
    return shared_library_info, cxx_resource_info, linkable_graph

def _create_non_template_providers(
    ctx: AnalysisContext,
    library_output: [JavaClasspathEntry, None],
    global_code_config,
    declared_deps: list[Dependency] = [],
    provided_deps: list[Dependency] = [],
    exported_deps: list[Dependency] = [],
    exported_provided_deps: list[Dependency] = [],
    runtime_deps: list[Dependency] = [],
    needs_desugar: bool = True,
    desugar_classpath: [TransitiveSetArgsProjection, None] = None,
    is_prebuilt_jar: bool = False,
    has_srcs: bool = True,
    sources_jar: Artifact | None = None,
    proguard_config: Artifact | None = None,
    gwt_module: Artifact | None = None,
    dex_weight_factor: int = 1,
) -> (JavaLibraryInfo, JavaPackagingInfo, JavaGlobalCodeInfo, SharedLibraryInfo, ResourceInfo, LinkableGraph):
    """Creates java library providers of type `JavaLibraryInfo` and `JavaPackagingInfo`.

    Args:
        library_output: optional JavaClasspathEntry that represents library output
        declared_deps: declared dependencies (usually comes from `deps` field of the rule)
        exported_deps: dependencies that are exposed to dependent rules as compiling deps
        exported_provided_deps: dependencies that are are exposed to dependent rules and not be included into packaging
        runtime_deps: dependencies that are used for packaging only
    """
    packaging_deps = declared_deps + exported_deps + runtime_deps
    for dep in packaging_deps:
        if JavaLibraryInfo in dep and dep[JavaLibraryInfo].may_not_be_packaged:
            fail(
                "{} has 'may_not_be_packaged' label but is present in {}. If you need to use it in order to build the library, move it into 'provided_deps'".format(
                    dep.label.raw_target(),
                    ctx.label.raw_target(),
                )
            )
    shared_library_info, cxx_resource_info, linkable_graph = create_native_providers(ctx, ctx.label, packaging_deps)

    output_for_classpath_macro = (
        library_output.abi
        if (library_output and library_output.abi.owner != None)
        else ctx.actions.write("dummy_output_for_classpath_macro.txt", "Unused", has_content_based_path = False)
    )
    java_packaging_dep = create_java_packaging_dep(
        ctx,
        library_output.full_library if library_output else None,
        output_for_classpath_macro,
        needs_desugar,
        desugar_classpath,
        is_prebuilt_jar,
        has_srcs,
        sources_jar,
        proguard_config = proguard_config,
        gwt_module = gwt_module,
        dex_weight_factor = dex_weight_factor,
        compiler_depth = library_output.full_library_compiler_depth if library_output else None,
    )

    java_packaging_info = get_java_packaging_info(
        ctx,
        raw_deps = packaging_deps,
        java_packaging_dep = java_packaging_dep,
    )

    single_library = single_library_compiling_deps(ctx.actions, library_output)

    single_library_wrapper = single_library_compiling_deps_wrapper(ctx.actions, single_library)
    compiling_deps_wrapper = derive_compiling_deps_wrapper(ctx.actions, single_library_wrapper, exported_deps + exported_provided_deps)
    first_order_compiling_deps_wrappers_without_library_itself = filter(
        None,
        [
            exported_dep.compiling_deps_wrapper
            for exported_dep in filter(None, [x.get(JavaLibraryInfo) for x in declared_deps + exported_deps + provided_deps + exported_provided_deps])
        ],
    )

    global_code_info = get_global_code_info(
        ctx,
        declared_deps,
        packaging_deps,
        single_library_wrapper,
        compiling_deps_wrapper,
        first_order_compiling_deps_wrappers_without_library_itself,
        global_code_config,
    )

    compiling_deps = get_compiling_deps_tset(
        ctx.actions,
        exported_deps + exported_provided_deps,
        [single_library] if single_library else [],
    )

    return (
        JavaLibraryInfo(
            compiling_deps = compiling_deps,
            compiling_deps_wrapper = compiling_deps_wrapper,
            library_output = library_output,
            output_for_classpath_macro = output_for_classpath_macro,
            may_not_be_exported = "may_not_be_exported" in (ctx.attrs.labels or []),
            may_not_be_packaged = "may_not_be_packaged" in (ctx.attrs.labels or []),
        ),
        java_packaging_info,
        global_code_info,
        shared_library_info,
        cxx_resource_info,
        linkable_graph,
    )

def create_template_info(ctx: AnalysisContext, packaging_info: JavaPackagingInfo, first_order_classpath_libs: list[Artifact]) -> TemplatePlaceholderInfo:
    return TemplatePlaceholderInfo(
        keyed_variables = {
            "classpath": cmd_args(packaging_info.packaging_deps.project_as_args("full_jar_args"), delimiter = get_path_separator_for_exec_os(ctx))
            if packaging_info.packaging_deps
            else cmd_args(),
            "classpath_including_targets_with_no_output": cmd_args(
                packaging_info.packaging_deps.project_as_args("args_for_classpath_macro"), delimiter = get_path_separator_for_exec_os(ctx)
            ),
            "first_order_classpath": cmd_args(first_order_classpath_libs, delimiter = get_path_separator_for_exec_os(ctx)),
        }
    )

def create_java_library_providers(
    ctx: AnalysisContext,
    library_output: [JavaClasspathEntry, None],
    global_code_config,
    declared_deps: list[Dependency] = [],
    exported_deps: list[Dependency] = [],
    provided_deps: list[Dependency] = [],
    exported_provided_deps: list[Dependency] = [],
    runtime_deps: list[Dependency] = [],
    needs_desugar: bool = True,
    is_prebuilt_jar: bool = False,
    has_srcs: bool = True,
    sources_jar: Artifact | None = None,
    generated_sources: list[Artifact] = [],
    annotation_jars_dir: Artifact | None = None,
    proguard_config: Artifact | None = None,
    gwt_module: Artifact | None = None,
    lint_jar: Artifact | None = None,
    preprocessed_library: Artifact | None = None,
    used_jars_json: Artifact | None = None,
    dex_weight_factor: int = 1,
) -> (JavaLibraryInfo, JavaPackagingInfo, JavaGlobalCodeInfo, SharedLibraryInfo, ResourceInfo, LinkableGraph, TemplatePlaceholderInfo, JavaLibraryIntellijInfo):
    first_order_classpath_deps = filter(None, [x.get(JavaLibraryInfo) for x in declared_deps + exported_deps + runtime_deps])
    first_order_classpath_libs = [dep.output_for_classpath_macro for dep in first_order_classpath_deps]

    compiling_deps = get_compiling_deps_tset(ctx.actions, declared_deps + exported_deps + provided_deps + exported_provided_deps)
    desugar_classpath = None
    if needs_desugar and compiling_deps != None and _is_dexing_enabled(ctx):
        # Below API 24, D8 desugars interface method calls and needs synthetic members that ABI jars drop.
        min_sdk_version = ctx.attrs.min_sdk_version
        abi_jars_suffice = min_sdk_version != None and int(min_sdk_version) >= 24
        desugar_classpath = compiling_deps.project_as_args("args_for_compiling" if abi_jars_suffice else "full_library_args")

    library_info, packaging_info, global_code_info, shared_library_info, cxx_resource_info, linkable_graph = _create_non_template_providers(
        ctx,
        library_output = library_output,
        global_code_config = global_code_config,
        declared_deps = declared_deps,
        provided_deps = provided_deps,
        exported_deps = exported_deps,
        exported_provided_deps = exported_provided_deps,
        runtime_deps = runtime_deps,
        needs_desugar = needs_desugar,
        desugar_classpath = desugar_classpath,
        is_prebuilt_jar = is_prebuilt_jar,
        has_srcs = has_srcs,
        sources_jar = sources_jar,
        proguard_config = proguard_config,
        gwt_module = gwt_module,
        dex_weight_factor = dex_weight_factor,
    )

    first_order_libs = first_order_classpath_libs + [library_info.library_output.full_library] if library_info.library_output else first_order_classpath_libs
    template_info = create_template_info(ctx, packaging_info, first_order_libs)

    intellij_info = JavaLibraryIntellijInfo(
        compiling_classpath = [dep.full_library for dep in (list(compiling_deps.traverse()) if compiling_deps else [])],
        generated_sources = generated_sources,
        annotation_jars_dir = annotation_jars_dir,
        lint_jar = lint_jar,
        preprocessed_library = preprocessed_library,
        used_jars_json = used_jars_json,
    )

    return (library_info, packaging_info, global_code_info, shared_library_info, cxx_resource_info, linkable_graph, template_info, intellij_info)
