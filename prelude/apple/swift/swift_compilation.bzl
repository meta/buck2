# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactInfoTag",
    "ArtifactTSet",  # @unused Used as a type
    "make_artifact_tset",
    "project_artifacts",
)
load("@prelude//:paths.bzl", "paths")
load("@prelude//apple:apple_error_handler.bzl", "apple_build_error_handler")
load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo", "AppleToolsInfo")
load("@prelude//apple:apple_utility.bzl", "get_apple_cxx_headers_layout", "get_disable_pch_validation_flags", "get_module_name")
load("@prelude//apple:modulemap.bzl", "create_modulemap")
load(
    "@prelude//apple/swift:swift_helpers.bzl",
    "compile_with_argsfile",
    "compile_with_argsfile_cmd",
    "get_swift_dynamic_thunk_entry_point_name",
    "uses_dynamic_replacement",
    "uses_explicit_modules",
)
load(
    "@prelude//apple/swift:swift_types.bzl",
    "EXPLICIT_MODULES_CATEGORY_SUFFIX",
    "SWIFTMODULE_COMPILE_CATEGORY",
    "SWIFTMODULE_EXTENSION",
    "SWIFT_COMPILE_CATEGORY",
    "SWIFT_EXTENSION",
    "SwiftDependencyInfo",
    "SwiftMacroPlugin",
    "get_implicit_framework_search_path_providers",
)
load("@prelude//cxx:argsfiles.bzl", "CompileArgsfile", "CompileArgsfiles")
load("@prelude//cxx:cxx_context.bzl", "get_cxx_platform_info", "get_cxx_toolchain_info")
load("@prelude//cxx:cxx_library_utility.bzl", "cxx_attr_deps", "cxx_attr_exported_deps")
load(
    "@prelude//cxx:cxx_sources.bzl",
    "CxxSrcWithFlags",  # @unused Used as a type
)
load("@prelude//cxx:cxx_utility.bzl", "cxx_attrs_get_allow_cache_upload")
load(
    "@prelude//cxx:headers.bzl",
    "CHeader",
    "HeaderMode",
    "cxx_attr_headers",
    "prepare_headers",
)
load(
    "@prelude//cxx:link_groups.bzl",
    "get_link_group",
)
load(
    "@prelude//cxx:preprocessor.bzl",
    "CPreprocessor",
    "CPreprocessorArgs",
    "CPreprocessorInfo",  # @unused Used as a type
    "cxx_inherited_preprocessor_infos",
    "cxx_merge_cpreprocessors",
)
load("@prelude//cxx:target_sdk_version.bzl", "get_target_triple")
load(
    "@prelude//linking:link_info.bzl",
    "LinkInfo",  # @unused Used as a type
    "SwiftmoduleLinkable",  # @unused Used as a type
)
load("@prelude//utils:actions.bzl", "ActionExecutionAttributes")
load("@prelude//utils:arglike.bzl", "ArgLike")
load(
    ":apple_sdk_modules_utility.bzl",
    "get_compiled_sdk_clang_deps_tset",
    "get_compiled_sdk_swift_deps_tset",
    "get_uncompiled_sdk_deps",
    "is_sdk_modules_provided",
)
load(
    ":swift_debug_info_utils.bzl",
    "extract_and_merge_clang_debug_infos",
    "extract_and_merge_swift_debug_infos",
)
load(
    ":swift_incremental_support.bzl",
    "INCREMENTAL_SWIFT_COMPILE_BATCH_SIZE",
    "INCREMENTAL_SWIFT_COMPILE_MAX_NUM_THREADS",
    "IncrementalCompilationInput",
    "get_incremental_file_hashing_enabled",
    "get_incremental_object_compilation_flags",
    "get_incremental_remote_outputs_enabled",
    "get_incremental_split_actions",
    "get_incremental_swiftmodule_compilation_flags",
    "get_uses_content_based_paths",
    "should_build_swift_incrementally",
    "should_build_swiftmodule_incrementally",
)
load(":swift_module_map.bzl", "write_swift_module_map_with_deps")
load(":swift_output_file_map.bzl", "add_dependencies_output", "get_modularization_dependency_graph_output_map")
load(":swift_pcm_compilation.bzl", "compile_internal_bridging_header_pch", "compile_underlying_pcm", "get_compiled_pcm_deps_tset", "get_swift_pcm_anon_targets")
load(
    ":swift_pcm_compilation_types.bzl",
    "SwiftPCMUncompiledInfo",
)
load(":swift_sdk_flags.bzl", "get_sdk_flags")
load(":swift_sdk_pcm_compilation.bzl", "get_swift_sdk_pcm_anon_targets")
load(":swift_swiftinterface_compilation.bzl", "get_swift_interface_anon_targets")
load(":swift_toolchain.bzl", "get_swift_toolchain_info", "include_path_for_relative_module_map_paths", "supports_modulemaps_with_hmaps")
load(
    ":swift_toolchain_types.bzl",
    "SwiftCompiledModuleInfo",
    "SwiftCompiledModuleTset",
    "SwiftObjectFormat",
    "SwiftToolchainInfo",
    "clang_module_file_args",
)

SwiftCompilationDatabase = record(
    db = field(Artifact),
    other_outputs = field(ArgLike),
)

# One file of the `@_dynamicReplacement` thunk module.
SwiftDynamicThunkSource = record(
    # The module source the file was generated from; None for the entry file.
    original = field(Artifact | None),
    thunk = field(Artifact),
)

SwiftDynamicThunkOutput = record(
    object = field(Artifact),
    module_name = field(str),
    sources = field(list[SwiftDynamicThunkSource]),
    # The commands that generated `sources` and compiled `object`, written
    # with their outputs as plain paths so they can be recorded and re-run.
    transform_command = field(cmd_args),
    compile_command = field(cmd_args),
    compile_argsfile = field(Artifact),
)

SwiftObjectOutput = record(
    object_files = field(list[Artifact]),
    argsfiles = field(CompileArgsfiles),
    output_map_artifact = field(Artifact | None),
    swiftdeps = field(list[Artifact]),
)

SwiftLibraryForDistributionOutput = record(
    swiftinterface = field(Artifact),
    private_swiftinterface = field(Artifact),
    swiftdoc = field(Artifact),
)

SwiftCompilationOutput = record(
    # The object file output from compilation.
    object_files = field(list[Artifact]),
    object_format = field(SwiftObjectFormat),
    # The swiftmodule file output from compilation.
    swiftmodule = field(Artifact),
    # Stderr output when running `-typecheck`
    typecheck_file = field(Artifact),
    # The dependency info provider that contains the swiftmodule
    # search paths required for compilation and linking.
    dependency_info = field(SwiftDependencyInfo),
    # Preprocessor info required for ObjC compilation of this library.
    pre = field(CPreprocessor),
    # Exported preprocessor info required for ObjC compilation of rdeps.
    exported_pre = field(CPreprocessor),
    # Exported -Swift.h header
    exported_swift_header = field(Artifact),
    # The umbrella header generated for a mixed module (`__<M>_umbrella__/<M>.h`, when
    # `disable_generated_umbrella_header` is off): the exported header map names it under
    # `<M>/<M>.h`, and `[umbrella-header]` is the one way to build it by label.
    generated_umbrella_header = field(Artifact | None, None),
    # Argsfiles used to compile object files.
    argsfiles = field(CompileArgsfiles),
    # A tset of (SDK/first-party) swiftmodule artifacts required to be linked into binary.
    # Different outputs of Swift compilation actions might contain duplicates of SDK swiftmodule artifacts.
    swift_debug_info = field(ArtifactTSet),
    # A tset of PCM artifacts used to compile a Swift module.
    clang_debug_info = field(ArtifactTSet),
    # Info required for `[swift-compilation-database]` subtarget.
    compilation_database = field(SwiftCompilationDatabase),
    # An artifact that represent the Swift module map for this target.
    output_map_artifact = field(Artifact | None),
    # The JSON file passed to swiftc via -explicit-swift-module-map-file that
    # describes all transitive Swift module dependencies. Only populated when
    # explicit modules are enabled.
    swift_module_map_artifact = field(Artifact | None, None),
    # The `@_dynamicReplacement` thunks for every function in the module,
    # compiled as a separate module that imports this one. Populated when
    # `swift_dynamic_replacement_enabled` is set and the toolchain carries the
    # thunk tool; `apple_library` links the object and records the commands in
    # `[swift-dynamic-dylib-thunk-command-database]`.
    dynamic_thunk = field(SwiftDynamicThunkOutput | None, None),
    # An optional artifact with files that support consuming the generated library with later versions of the swift compiler.
    swift_library_for_distribution_output = field(SwiftLibraryForDistributionOutput | None),
    # A list of artifacts that stores the index data
    index_store = field(Artifact),
    # A list of artifacts of the swiftdeps files produced during incremental compilation.
    swiftdeps = field(list[Artifact]),
    compiled_underlying_pcm_artifact = field(Artifact | None),
    # Modularization dependency dot graph
    modularization_dependency_graph = field(Artifact | None),
)

SwiftCompileResult = record(
    swift_compilation = field(SwiftCompilationOutput | None),
    objc_swift_interface = field(DefaultInfo),
    swiftinterface = field(Artifact | None),
    ast = field(Artifact | None),
)

SwiftDebugInfo = record(
    static = list[ArtifactTSet],
    shared = list[ArtifactTSet],
)

_REQUIRED_SDK_MODULES = ["Swift", "SwiftOnoneSupport", "Darwin", "_Concurrency", "_StringProcessing"]

# Makes every function in the module replaceable at run time with
# `@_dynamicReplacement`, lets another module import its private declarations
# with `@_private(sourceFile:)`, and lets later replacements chain on earlier
# ones. These go into the shared flags so the swiftmodule, which is emitted by
# a separate action in WMO mode, records the declarations as `dynamic` and
# serializes the private ones.
_DYNAMIC_REPLACEMENT_FLAGS = [
    "-Xfrontend",
    "-enable-implicit-dynamic",
    "-Xfrontend",
    "-enable-private-imports",
    "-Xfrontend",
    "-enable-dynamic-replacement-chaining",
]

# Result builders that protocols' `body` requirements carry, which a thunk has
# to restate because a replacement does not inherit them: `Protocol=Builder`
# pairs, comma separated, from `apple.swift_dynamic_thunk_result_builders`.
# SwiftUI's `View` is always known to the tool.
def _parse_result_builders(value: str) -> dict[str, str]:
    builders = {}
    for pair in value.split(","):
        pair = pair.strip()
        if not pair:
            continue
        parts = [part.strip() for part in pair.split("=")]
        if len(parts) != 2 or not parts[0] or not parts[1]:
            fail("apple.swift_dynamic_thunk_result_builders expects comma separated `Protocol=Builder` pairs, got `{}`".format(pair))
        builders[parts[0]] = parts[1]
    return builders

_DYNAMIC_THUNK_RESULT_BUILDERS = _parse_result_builders(read_root_config("apple", "swift_dynamic_thunk_result_builders", ""))

_REQUIRED_SDK_CXX_MODULES = _REQUIRED_SDK_MODULES + ["std"]

_INDEX_SYSTEM_MODULES = read_root_config("swift", "index_system_modules", "false").lower() == "true"

def _get_target_flags(ctx) -> list[str]:
    if get_cxx_platform_info(ctx).name.startswith("linux"):
        # Linux triples are unversioned
        return ["-target", "{}-unknown-linux-gnu".format(get_swift_toolchain_info(ctx).architecture)]
    else:
        return ["-target", get_target_triple(ctx)]

def get_swift_anonymous_targets_for_prebuilt_framework(ctx: AnalysisContext, get_providers: typing.Callable) -> Promise:
    # Get SDK deps from direct dependencies,
    # all transitive deps will be compiled recursively.
    direct_uncompiled_sdk_deps = get_uncompiled_sdk_deps(
        ctx.attrs.sdk_modules,
        _REQUIRED_SDK_MODULES,
        get_swift_toolchain_info(ctx),
    )

    # Recursively compiling headers of direct and transitive deps as PCM modules,
    # prebuilt_apple_framework rule doesn't support custom compiler_flags, so we pass only target triple.
    pcm_targets = get_swift_pcm_anon_targets(
        ctx,
        ctx.attrs.deps,
        _get_target_flags(ctx),
        False,  # C++ Interop is disabled for now.
    )

    # Recursively compiling SDK's Clang dependencies,
    # prebuilt_apple_framework rule doesn't support custom compiler_flags, so we pass only target triple.
    sdk_pcm_targets = get_swift_sdk_pcm_anon_targets(
        ctx,
        False,
        direct_uncompiled_sdk_deps,
        _get_target_flags(ctx),
    )

    # Recursively compile SDK and prebuilt_apple_framework's Swift dependencies.
    swift_interface_anon_targets = get_swift_interface_anon_targets(
        ctx,
        direct_uncompiled_sdk_deps,
    )
    return ctx.actions.anon_targets(pcm_targets + sdk_pcm_targets + swift_interface_anon_targets).promise.map(get_providers)

def get_swift_anonymous_targets(ctx: AnalysisContext, get_apple_library_providers: typing.Callable) -> Promise:
    swift_cxx_flags = get_swift_cxx_flags(ctx)

    # Get SDK deps from direct dependencies,
    # all transitive deps will be compiled recursively.
    direct_uncompiled_sdk_deps = get_uncompiled_sdk_deps(
        ctx.attrs.sdk_modules,
        _REQUIRED_SDK_CXX_MODULES if ctx.attrs.enable_cxx_interop else _REQUIRED_SDK_MODULES,
        get_swift_toolchain_info(ctx),
    )

    # Recursively compiling headers of direct and transitive deps as PCM modules,
    # passing apple_library's cxx flags through that must be used for all downward PCM compilations.
    pcm_targets = get_swift_pcm_anon_targets(
        ctx,
        ctx.attrs.deps + getattr(ctx.attrs, "exported_deps", []),
        swift_cxx_flags,
        ctx.attrs.enable_cxx_interop,
    )

    # Recursively compiling SDK's Clang dependencies,
    # passing apple_library's cxx flags through that must be used for all downward PCM compilations.
    sdk_pcm_targets = get_swift_sdk_pcm_anon_targets(
        ctx,
        ctx.attrs.enable_cxx_interop,
        direct_uncompiled_sdk_deps,
        swift_cxx_flags,
    )

    # Recursively compiling SDK's Swift dependencies,
    # passing apple_library's cxx flags through that must be used for all downward PCM compilations.
    swift_interface_anon_targets = get_swift_interface_anon_targets(
        ctx,
        direct_uncompiled_sdk_deps,
    )
    return ctx.actions.anon_targets(pcm_targets + sdk_pcm_targets + swift_interface_anon_targets).promise.map(get_apple_library_providers)

def get_swift_cxx_flags(ctx: AnalysisContext) -> list[str]:
    """Iterates through `swift_compiler_flags` and returns a list of flags that might affect Clang compilation"""
    gather, next = ([], False)

    # Each target needs to propagate the compilers target triple.
    # This can vary depending on the deployment target of each library.
    gather += _get_target_flags(ctx)

    for f in ctx.attrs.swift_compiler_flags:
        if next:
            gather.append("-Xcc")
            gather.append(str(f).replace('"', ""))
            next = False
        elif str(f) == '"-Xcc"':
            next = True
        elif str(f) == '"-warnings-as-errors"':
            gather.append("-warnings-as-errors")
        elif str(f) == '"-no-warnings-as-errors"':
            gather.append("-no-warnings-as-errors")

    if ctx.attrs.enable_cxx_interop:
        gather += ["-cxx-interoperability-mode=default"]

    gather += ["-swift-version", ctx.attrs.swift_version]

    return gather

def _get_pcm_preprocessor_info(
    ctx: AnalysisContext,
    headers: list[CHeader],
    preprocessor_infos: list[CPreprocessorInfo],
    header_map_name: str,
) -> CPreprocessorInfo:
    header_dict = {paths.join(h.namespace, h.name): h.artifact for h in headers}
    header_map = prepare_headers(
        ctx.actions,
        get_cxx_toolchain_info(ctx),
        header_dict,
        header_map_name,
        HeaderMode("header_map_only"),
        allow_cache_upload = cxx_attrs_get_allow_cache_upload(ctx.attrs),
    )
    header_map_args = cmd_args(header_map.include_path, format = "-I{}") if header_map else cmd_args()
    return cxx_merge_cpreprocessors(
        ctx.actions,
        [CPreprocessor(args = CPreprocessorArgs(args = [header_map_args]))],
        preprocessor_infos,
    )

def _get_internal_bridging_header_flags(
    ctx: AnalysisContext,
    module_name: str,
    public_preprocessor_info: CPreprocessorInfo,
    uncompiled_pcm_info: SwiftPCMUncompiledInfo | None,
    public_module: SwiftCompiledModuleInfo | None,
    deps_providers: list,
    swift_cxx_flags: list[str],
    framework_search_paths: cmd_args,
) -> cmd_args:
    header = getattr(ctx.attrs, "internal_bridging_header", None)
    if header == None:
        return cmd_args()

    exported_dep_labels = {dep.label: None for dep in getattr(ctx.attrs, "exported_deps", [])}
    private_deps = [dep for dep in ctx.attrs.deps if dep.label not in exported_dep_labels]
    private_headers = cxx_attr_headers(ctx, get_apple_cxx_headers_layout(ctx))
    private_header_args = cmd_args()
    if private_headers or private_deps:
        private_preprocessor_info = _get_pcm_preprocessor_info(
            ctx,
            private_headers,
            cxx_inherited_preprocessor_infos(private_deps),
            "swift-internal-bridging-header-headers",
        )
        private_header_args.add(cmd_args(private_preprocessor_info.set.project_as_args("args"), prepend = "-Xcc"))
    public_header_args = cmd_args(public_preprocessor_info.set.project_as_args("args"), prepend = "-Xcc")
    if not uses_explicit_modules(ctx):
        return cmd_args("-internal-import-bridging-header", header, private_header_args, public_header_args)

    if uncompiled_pcm_info == None:
        fail("Explicit internal bridging-header compilation requires SDK module information")
    additional_args = cmd_args(framework_search_paths, private_header_args)
    if not supports_modulemaps_with_hmaps(ctx):
        additional_args.add(public_header_args)
    if public_module:
        additional_args.add(public_module.clang_importer_args, clang_module_file_args(public_module))
    pch = compile_internal_bridging_header_pch(
        ctx,
        module_name,
        header,
        uncompiled_pcm_info,
        deps_providers,
        swift_cxx_flags,
        additional_args,
    )
    # Clang reads the PCH's input headers when loading it, even with PCH
    # validation disabled, so they must be materialized for Swift compilation.
    return cmd_args(
        "-disable-bridging-pch",
        "-internal-import-bridging-header",
        pch,
        hidden = [header, private_header_args, public_header_args],
    )

def _generate_umbrella_header(ctx: AnalysisContext, module_name: str, exported_headers: list[CHeader], uses_content_based_paths: bool) -> [CHeader, None]:
    """Auto-generate an umbrella header for mixed ObjC/Swift modules.

    The -Swift.h generated by the compiler may contain
    `#import <ModuleName/ModuleName.h>`. If no manually-created umbrella
    header exists, we generate one that #imports all exported ObjC headers
    so that import resolves correctly.
    """
    umbrella_header_name = module_name + ".h"
    umbrella_import_path = module_name + "/" + umbrella_header_name
    for h in exported_headers:
        if h.namespace:
            header_import_path = h.namespace + "/" + h.name
        else:
            header_import_path = h.name
        if header_import_path == umbrella_import_path:
            return None

    umbrella_content = "// Auto-generated umbrella header for {}\n".format(module_name)
    for h in exported_headers:
        if h.namespace:
            umbrella_content += "#import <{}/{}>\n".format(h.namespace, h.name)
        else:
            umbrella_content += "#import <{}/{}>\n".format(module_name, h.name)
    umbrella_artifact = ctx.actions.declare_output(
        "__{}_umbrella__/{}".format(module_name, umbrella_header_name),
        has_content_based_path = uses_content_based_paths,
    )
    ctx.actions.write(umbrella_artifact.as_output(), umbrella_content)
    return CHeader(
        artifact = umbrella_artifact,
        name = umbrella_header_name,
        namespace = module_name,
        named = False,
    )

def compile_swift(
    ctx: AnalysisContext,
    srcs: list[CxxSrcWithFlags],
    parse_as_library: bool,
    deps_providers: list,
    module_name: str,
    exported_headers: list[CHeader],
    exported_objc_modulemap_pp_info: [CPreprocessor, None],
    framework_search_paths_flags: cmd_args,
    extra_search_paths_flags: list[ArgLike] = [],
    compile_category = SWIFT_COMPILE_CATEGORY,
    compile_swiftmodule_category = SWIFTMODULE_COMPILE_CATEGORY,
    is_macro = False,
) -> SwiftCompileResult:
    # If this target imports XCTest we need to pass the search path to its swiftmodule.
    framework_search_paths = cmd_args()
    framework_search_paths.add(_get_xctest_swiftmodule_search_path(ctx))

    uses_content_based_paths = get_uses_content_based_paths(ctx)

    # Pass the framework search paths to the driver and clang importer. This is required
    # for pcm compilation, which does not pass through driver search paths.
    framework_search_paths.add(framework_search_paths_flags)
    framework_search_paths.add(cmd_args(framework_search_paths_flags, prepend = "-Xcc"))

    # If a target exports ObjC headers and Swift explicit modules are enabled,
    # we need to precompile a PCM of the underlying module and supply it to the Swift compilation.
    swift_cxx_flags = get_swift_cxx_flags(ctx)
    explicit_modules_enabled = uses_explicit_modules(ctx)
    internal_bridging_header = getattr(ctx.attrs, "internal_bridging_header", None)
    public_preprocessor_info = None
    if internal_bridging_header != None or (explicit_modules_enabled and exported_objc_modulemap_pp_info and supports_modulemaps_with_hmaps(ctx)):
        public_preprocessor_info = _get_pcm_preprocessor_info(
            ctx,
            exported_headers,
            cxx_inherited_preprocessor_infos(getattr(ctx.attrs, "exported_deps", [])),
            "swift-underlying-headers",
        )
    uncompiled_pcm_info = None
    if explicit_modules_enabled and (exported_objc_modulemap_pp_info or internal_bridging_header != None):
        uncompiled_pcm_info = get_swift_pcm_uncompile_info(
            ctx,
            public_preprocessor_info if supports_modulemaps_with_hmaps(ctx) else None,
            exported_objc_modulemap_pp_info,
        )
    exported_compiled_underlying_pcm = None
    if exported_objc_modulemap_pp_info and uncompiled_pcm_info:
        exported_compiled_underlying_pcm = compile_underlying_pcm(
            ctx,
            module_name,
            uncompiled_pcm_info,
            deps_providers,
            swift_cxx_flags,
            framework_search_paths,
        )

    # We always track inputs for dependency file tracking, but optionally set
    # the tag on the action depending on the use_depsfiles config.
    inputs_tag = ctx.actions.artifact_tag()

    internal_bridging_header_flags = cmd_args()
    if internal_bridging_header != None:
        internal_bridging_header_flags = _get_internal_bridging_header_flags(
            ctx,
            module_name,
            public_preprocessor_info,
            uncompiled_pcm_info,
            exported_compiled_underlying_pcm,
            deps_providers,
            swift_cxx_flags,
            framework_search_paths,
        )
    shared_flags, swift_module_map_artifact = _get_shared_flags(
        ctx = ctx,
        deps_providers = deps_providers,
        parse_as_library = parse_as_library,
        public_module = exported_compiled_underlying_pcm,
        module_name = module_name,
        public_modulemap_pp_info = exported_objc_modulemap_pp_info,
        extra_search_paths_flags = extra_search_paths_flags,
        inputs_tag = inputs_tag,
        is_macro = is_macro,
    )
    shared_flags.add(internal_bridging_header_flags)
    shared_flags.add(framework_search_paths)
    objc_swift_interface_info = _create_objc_swift_interface(ctx, shared_flags, module_name)

    if not srcs:
        return SwiftCompileResult(swift_compilation = None, objc_swift_interface = objc_swift_interface_info, swiftinterface = None, ast = None)

    # Content-based path hashing and Swift incremental compilation are incompatible.
    if uses_content_based_paths and should_build_swift_incrementally(ctx):
        fail("Content-based path hashing and Swift incremental compilation cannot be enabled at the same time.")

    toolchain = get_swift_toolchain_info(ctx)
    output_header = ctx.actions.declare_output(module_name + "-Swift.h", has_content_based_path = uses_content_based_paths)
    output_swiftmodule = ctx.actions.declare_output(module_name + SWIFTMODULE_EXTENSION, has_content_based_path = uses_content_based_paths)

    swift_framework_output = None
    output_swiftinterface = None
    swiftinterface_subtarget_enabled = getattr(ctx.attrs, "swiftinterface_subtarget_enabled", False)

    if _should_compile_with_evolution(ctx):
        swift_framework_output = SwiftLibraryForDistributionOutput(
            swiftinterface = ctx.actions.declare_output(module_name + ".swiftinterface", has_content_based_path = uses_content_based_paths),
            private_swiftinterface = ctx.actions.declare_output(module_name + ".private.swiftinterface", has_content_based_path = uses_content_based_paths),
            swiftdoc = ctx.actions.declare_output(
                module_name + ".swiftdoc", has_content_based_path = uses_content_based_paths
            ),  # this is generated automatically once we pass -emit-module-info, so must have this name
        )
        if swiftinterface_subtarget_enabled:
            output_swiftinterface = swift_framework_output.swiftinterface
    elif swiftinterface_subtarget_enabled:
        output_swiftinterface = ctx.actions.declare_output(module_name + ".swiftinterface", has_content_based_path = uses_content_based_paths)
        _compile_swiftinterface(
            ctx,
            toolchain,
            shared_flags,
            srcs,
            output_swiftinterface,
        )

    output_ast = None
    if getattr(ctx.attrs, "swift_dump_ast_subtarget_enabled", False):
        output_ast = ctx.actions.declare_output(module_name + "-ast-dump", dir = True, has_content_based_path = uses_content_based_paths)
        _compile_dump_ast(
            ctx,
            toolchain,
            shared_flags,
            srcs,
            output_ast,
        )

    # When compiling with WMO or incremental with split actions enabled, we compile
    # the swiftmodule separately. In pure incremental mode, we generate the swiftmodule
    # as part of the compile action to make use of incrementality.
    if not should_build_swift_incrementally(ctx) or get_incremental_split_actions(ctx):
        _compile_swiftmodule(
            ctx,
            toolchain,
            shared_flags,
            srcs,
            output_swiftmodule,
            output_header,
            swift_framework_output,
            inputs_tag,
            compile_swiftmodule_category,
        )

    object_output = _compile_object(
        ctx = ctx,
        toolchain = toolchain,
        shared_flags = shared_flags,
        srcs = srcs,
        output_swiftmodule = output_swiftmodule,
        output_header = output_header,
        inputs_tag = inputs_tag,
        category = compile_category,
    )

    dynamic_thunk = None
    if uses_dynamic_replacement(ctx) and not is_macro:
        dynamic_thunk = _compile_dynamic_thunk(
            ctx = ctx,
            toolchain = toolchain,
            deps_providers = deps_providers,
            module_name = module_name,
            srcs = srcs,
            output_swiftmodule = output_swiftmodule,
            underlying_module = exported_compiled_underlying_pcm,
            public_modulemap_pp_info = exported_objc_modulemap_pp_info,
            extra_search_paths_flags = extra_search_paths_flags,
            framework_search_paths = framework_search_paths,
            inputs_tag = inputs_tag,
        )

    typecheck_file = _compile_typecheck_diagnostics(
        ctx = ctx,
        toolchain = toolchain,
        shared_flags = shared_flags,
        srcs = srcs,
    )

    modularization_dependency_graph = _compile_modularization_dependency_graph(ctx, toolchain, shared_flags, srcs)

    index_store = _compile_index_store(ctx, toolchain, shared_flags, srcs)

    generated_umbrella_header = (
        _generate_umbrella_header(
            ctx,
            module_name,
            exported_headers,
            uses_content_based_paths,
        )
        if exported_headers and not getattr(ctx.attrs, "disable_generated_umbrella_header", True)
        else None
    )

    # Swift libraries extend the ObjC modulemaps to include the -Swift.h header
    modulemap_pp_info = create_modulemap(
        ctx,
        name = "swift-extended",
        module_name = module_name,
        headers = exported_headers,
        swift_header = output_header,
    )
    exported_swift_header = CHeader(
        artifact = output_header,
        name = output_header.basename,
        namespace = module_name,
        named = False,
    )
    modulemap_artifact = modulemap_pp_info.modulemap_artifact
    if generated_umbrella_header:
        modulemap_artifact = modulemap_artifact.with_associated_artifacts([generated_umbrella_header.artifact])
    exported_pp_info = CPreprocessor(
        headers = [exported_swift_header] + ([generated_umbrella_header] if generated_umbrella_header else []),
        modular_args = modulemap_pp_info.modular_args,
        args = CPreprocessorArgs(args = modulemap_pp_info.args.args),
        modulemap_artifact = modulemap_artifact,
    )

    # We also need to include the unprefixed -Swift.h header in this libraries preprocessor info
    swift_header = CHeader(
        artifact = output_header,
        name = output_header.basename,
        namespace = "",
        named = False,
    )
    pre = CPreprocessor(headers = [swift_header])

    clang_debug_artifacts = [exported_pp_info.modulemap_artifact]
    if exported_compiled_underlying_pcm:
        # The compiled PCM of the underlying clang module
        clang_debug_artifacts.append(exported_compiled_underlying_pcm.output_artifact)

        # The modulemap and headers excluding the Swift header
        clang_debug_artifacts.append(exported_objc_modulemap_pp_info.modulemap_artifact)

    # Pass up the swiftmodule paths for this module and its exported_deps
    return SwiftCompileResult(
        swift_compilation = SwiftCompilationOutput(
            output_map_artifact = object_output.output_map_artifact,
            swift_module_map_artifact = swift_module_map_artifact,
            object_files = object_output.object_files,
            object_format = toolchain.object_format,
            swiftmodule = output_swiftmodule,
            typecheck_file = typecheck_file,
            compiled_underlying_pcm_artifact = exported_compiled_underlying_pcm.output_artifact if exported_compiled_underlying_pcm else None,
            dependency_info = get_swift_dependency_info(
                ctx,
                output_swiftmodule,
                deps_providers,
                is_macro,
                swift_ast_dump_artifacts = filter(None, [output_ast]),
            ),
            pre = pre,
            exported_pre = exported_pp_info,
            exported_swift_header = exported_swift_header.artifact,
            generated_umbrella_header = generated_umbrella_header.artifact if generated_umbrella_header else None,
            argsfiles = object_output.argsfiles,
            swift_debug_info = extract_and_merge_swift_debug_infos(ctx, deps_providers, [output_swiftmodule]),
            clang_debug_info = extract_and_merge_clang_debug_infos(
                ctx,
                deps_providers,
                clang_debug_artifacts,
            ),
            compilation_database = _create_compilation_database(ctx, srcs, object_output.argsfiles.relative[SWIFT_EXTENSION]),
            swift_library_for_distribution_output = swift_framework_output,
            index_store = index_store,
            swiftdeps = object_output.swiftdeps,
            modularization_dependency_graph = modularization_dependency_graph,
            dynamic_thunk = dynamic_thunk,
        ),
        objc_swift_interface = objc_swift_interface_info,
        swiftinterface = output_swiftinterface,
        ast = output_ast,
    )

def _compile_swiftinterface(
    ctx: AnalysisContext, toolchain: SwiftToolchainInfo, shared_flags: cmd_args, srcs: list[CxxSrcWithFlags], output_swiftinterface: Artifact
):
    swiftinterface_argsfile = cmd_args(shared_flags)
    swiftinterface_argsfile.add([
        # Required as emitting a swiftinterface without library evolution
        # produces a warning.
        "-no-warnings-as-errors",
        # Workaround to avoid producing swiftdoc and other auxiliary outputs.
        "-typecheck",
        "-wmo",
    ])
    swiftinterface_cmd = cmd_args([
        "-emit-module-interface",
        "-emit-module-interface-path",
        output_swiftinterface.as_output(),
        "-Xwrapper",
        "-remove-module-prefixes",
    ])
    _compile_with_argsfile(
        ctx = ctx,
        category = "emit_swiftinterface",
        shared_flags = swiftinterface_argsfile,
        srcs = srcs,
        additional_flags = swiftinterface_cmd,
        toolchain = toolchain,
        supports_serialized_errors = False,
    )

def _compile_dump_ast(ctx: AnalysisContext, toolchain: SwiftToolchainInfo, shared_flags: cmd_args, srcs: list[CxxSrcWithFlags], output_ast_dir: Artifact):
    ast_output = output_ast_dir.as_output()
    output_file_map = {}
    for src in srcs:
        # safe_name includes path in case basenames clash
        safe_name = src.file.short_path.replace("/", "_")
        output_file_map[src.file] = {
            "ast-dump": cmd_args(ast_output, format = "{}/" + safe_name + ".ast", delimiter = ""),
        }

    argfile_cmd = cmd_args(shared_flags)
    if ctx.attrs.swift_module_skip_function_bodies:
        argfile_cmd.add([
            "-Xfrontend",
            "-experimental-skip-non-inlinable-function-bodies-without-types",
        ])

    additional_flags = cmd_args(["-dump-ast"])
    additional_flags.add(cmd_args(hidden = ast_output))

    # `-dump-ast` disables `-wmo`. Without an explicit `-j`, those jobs run
    # serially over each file in larger modules, becoming potential bottlenecks.
    additional_flags.add([
        "-enable-batch-mode",
        "-driver-batch-size-limit",
        str(INCREMENTAL_SWIFT_COMPILE_BATCH_SIZE),
        "-j",
        str(INCREMENTAL_SWIFT_COMPILE_MAX_NUM_THREADS),
    ])

    _compile_with_argsfile(
        ctx = ctx,
        category = "dump_ast",
        shared_flags = argfile_cmd,
        srcs = srcs,
        additional_flags = additional_flags,
        toolchain = toolchain,
        num_threads = INCREMENTAL_SWIFT_COMPILE_MAX_NUM_THREADS,
        output_file_map = output_file_map,
        supports_serialized_errors = False,
        incremental_build_allowed = False,
    )

def _dynamic_thunk_module_name(module_name: str) -> str:
    return module_name + "_DynamicThunk"

def _compile_dynamic_thunk(
    ctx: AnalysisContext,
    toolchain: SwiftToolchainInfo,
    deps_providers: list,
    module_name: str,
    srcs: list[CxxSrcWithFlags],
    output_swiftmodule: Artifact,
    underlying_module: SwiftCompiledModuleInfo | None,
    public_modulemap_pp_info: CPreprocessor | None,
    extra_search_paths_flags: list[ArgLike],
    framework_search_paths: cmd_args,
    inputs_tag: ArtifactTag,
) -> SwiftDynamicThunkOutput | None:
    """Compiles `@_dynamicReplacement` thunks for every function in the module.

    The thunk is its own module, `<Module>_DynamicThunk`, built with the same
    shared flags as the module and importing the module's swiftmodule: its
    replacement records must refer to the module's replacement keys as
    undefined symbols, which `dlopen` then binds to the running app's copies.
    Compiling the module's own sources a second time would define those keys
    locally and replace nothing anyone calls."""
    apple_tools = getattr(ctx.attrs, "_apple_tools", None)
    tool = apple_tools[AppleToolsInfo].make_swift_dynamic_thunk if apple_tools else None
    if not tool:
        return None

    uses_content_based_paths = get_uses_content_based_paths(ctx)
    thunk_module_name = _dynamic_thunk_module_name(module_name)
    entry_src = ctx.actions.declare_output(thunk_module_name, "__entry.thunk.swift", has_content_based_path = uses_content_based_paths)
    transform_args = [
        tool,
        "--module-name",
        module_name,
        "--entry-symbol",
        get_swift_dynamic_thunk_entry_point_name(ctx),
    ]
    for protocol, builder in _DYNAMIC_THUNK_RESULT_BUILDERS.items():
        transform_args.extend(["--result-builder", protocol + "=" + builder])

    # Each generated file is an output of the action and a plain path in the
    # recorded command, so the pairs are built once and projected twice.
    outputs = [("--entry-output", None, entry_src)]
    for src in srcs:
        # The short path keeps sources with the same basename apart. The
        # `.thunk.swift` suffix keeps the generated file's own `#fileID` apart
        # from the one its `#sourceLocation` directives produce for the
        # original, which the compiler requires to differ.
        thunk_src = ctx.actions.declare_output(
            thunk_module_name,
            paths.replace_extension(src.file.short_path.replace("/", "_"), ".thunk.swift"),
            has_content_based_path = uses_content_based_paths,
        )
        outputs.append(("--output", src.file, thunk_src))
    transform = cmd_args(transform_args)
    transform_command = cmd_args(transform_args)
    sources = []
    for flag, original, thunk_src in outputs:
        source_flags = ["--source", original] if original else []
        transform.add(source_flags, flag, thunk_src.as_output())
        transform_command.add(source_flags, flag, thunk_src)
        sources.append(SwiftDynamicThunkSource(original = original, thunk = thunk_src))
    thunk_srcs = [CxxSrcWithFlags(file = source.thunk) for source in sources]
    ctx.actions.run(
        transform,
        category = "swift_dynamic_thunk_sources",
        identifier = module_name,
        error_handler = apple_build_error_handler,
    )

    shared_flags, _ = _get_shared_flags(
        ctx = ctx,
        deps_providers = deps_providers,
        parse_as_library = True,
        public_module = underlying_module,
        module_name = thunk_module_name,
        public_modulemap_pp_info = public_modulemap_pp_info,
        extra_search_paths_flags = extra_search_paths_flags,
        inputs_tag = inputs_tag,
        is_macro = False,
        thunk_of_module = SwiftCompiledModuleInfo(
            is_framework = False,
            is_sdk_module = False,
            is_swiftmodule = True,
            module_name = module_name,
            output_artifact = output_swiftmodule,
        ),
    )
    shared_flags.add(framework_search_paths)

    output_object = ctx.actions.declare_output(thunk_module_name + ".o", has_content_based_path = uses_content_based_paths)
    argsfiles, _ = _compile_with_argsfile(
        ctx = ctx,
        category = "swift_dynamic_thunk_compile",
        shared_flags = shared_flags,
        srcs = thunk_srcs,
        additional_flags = cmd_args(["-emit-object", "-o", output_object.as_output(), "-wmo"]),
        toolchain = toolchain,
        extension = SWIFT_EXTENSION,
        incremental_build_allowed = False,
    )
    argsfile = argsfiles.relative[SWIFT_EXTENSION]
    return SwiftDynamicThunkOutput(
        object = output_object,
        module_name = thunk_module_name,
        sources = sources,
        transform_command = transform_command,
        # The same invocation as the action, less the wrapper's diagnostics
        # outputs: the argsfile's hidden inputs carry every module it imports.
        compile_command = cmd_args(toolchain.compiler, "-emit-object", "-o", output_object, "-wmo", argsfile.cmd_form, [src.file for src in thunk_srcs]),
        compile_argsfile = argsfile.file,
    )

# We use separate actions for swiftmodule and object file output. This
# improves build parallelism at the cost of duplicated work, but by disabling
# type checking in function bodies the swiftmodule compilation can be done much
# faster than object file output.
def _compile_swiftmodule(
    ctx: AnalysisContext,
    toolchain: SwiftToolchainInfo,
    shared_flags: cmd_args,
    srcs: list[CxxSrcWithFlags],
    output_swiftmodule: Artifact,
    output_header: Artifact,
    swift_framework_output: SwiftLibraryForDistributionOutput | None,
    inputs_tag: ArtifactTag,
    category: str,
) -> CompileArgsfiles:
    # The incremental swiftmodule action is opt-in and only supported when
    # we are not emitting library evolution outputs (which require WMO).
    build_swiftmodule_incrementally = should_build_swiftmodule_incrementally(ctx) and swift_framework_output == None

    argfile_cmd = cmd_args(shared_flags)
    argfile_cmd.add("-disable-cmo")

    # Shared between WMO and incremental swiftmodule actions: emit the
    # `.swiftmodule` + generated ObjC header, set up the include path for
    # relative module map paths, and (when requested) skip non-inlinable
    # function bodies for faster swiftmodule emission.
    cmd = cmd_args(
        "-emit-objc-header",
        "-emit-objc-header-path",
        output_header.as_output(),
        "-emit-module",
        "-emit-module-path",
        output_swiftmodule.as_output(),
    )
    cmd.add(include_path_for_relative_module_map_paths(ctx))
    if ctx.attrs.swift_module_skip_function_bodies:
        argfile_cmd.add([
            "-Xfrontend",
            "-experimental-skip-non-inlinable-function-bodies-without-types",
        ])

    output_file_map = {}
    incremental_artifacts = None
    skip_incremental_outputs = False

    if build_swiftmodule_incrementally:
        incremental_compilation_output = get_incremental_swiftmodule_compilation_flags(ctx, srcs)
        cmd.add(incremental_compilation_output.incremental_flags_cmd)
        output_file_map = incremental_compilation_output.output_file_map
        skip_incremental_outputs = incremental_compilation_output.skip_incremental_outputs
        incremental_artifacts = IncrementalCompilationInput(
            depfiles = incremental_compilation_output.depfiles,
            swiftdeps = incremental_compilation_output.swiftdeps,
            swiftdoc = None,
        )
    else:
        argfile_cmd.add([
            "-wmo",
        ])

        if swift_framework_output:
            argfile_cmd.add([
                "-enable-library-evolution",
            ])
            cmd.add([
                "-emit-module-interface",
                "-emit-module-interface-path",
                swift_framework_output.swiftinterface.as_output(),
                "-emit-private-module-interface-path",
                swift_framework_output.private_swiftinterface.as_output(),
                # Module verification fails here as the clang modules
                # are not loaded correctly.
                "-no-verify-emitted-module-interface",
            ])

            # There is no driver flag to specify the swiftdoc output path
            # TODO: use an output file map for this.
            cmd.add(cmd_args(hidden = swift_framework_output.swiftdoc.as_output()))

    dep_files = {}
    # With -skip-incremental-output the output_file_map is an output, so we
    # cannot support depsfiles. With incremental file hashing the dep_file
    # is also unnecessary.
    emit_depsfiles = (
        toolchain.use_depsfiles and not skip_incremental_outputs and not (build_swiftmodule_incrementally and get_incremental_file_hashing_enabled(ctx))
    )
    if emit_depsfiles:
        add_dependencies_output(ctx, output_file_map, cmd, "swiftmodule", inputs_tag)
        dep_files["swiftmodule"] = inputs_tag

    ret, _ = _compile_with_argsfile(
        ctx = ctx,
        category = category,
        extension = SWIFTMODULE_EXTENSION,
        shared_flags = argfile_cmd,
        srcs = srcs,
        additional_flags = cmd,
        toolchain = toolchain,
        num_threads = 1,
        dep_files = dep_files,
        output_file_map = output_file_map,
        artifact_tag = inputs_tag,
        incremental_build_allowed = build_swiftmodule_incrementally,
        skip_incremental_outputs = skip_incremental_outputs,
        objects = [],
        incremental_artifacts = incremental_artifacts,
    )
    return ret

def _compile_typecheck_diagnostics(ctx: AnalysisContext, toolchain: SwiftToolchainInfo, shared_flags: cmd_args, srcs: list[CxxSrcWithFlags]) -> Artifact:
    category = "swift_typecheck"
    uses_content_based_paths = get_uses_content_based_paths(ctx)

    if uses_explicit_modules(ctx):
        category += "_with_explicit_mods"

    additional_flags = cmd_args([
        "-typecheck",
        "-wmo",
    ])

    swift_cmd_output = compile_with_argsfile_cmd(
        ctx = ctx,
        category = category,
        shared_flags = shared_flags,
        srcs = srcs,
        additional_flags = additional_flags,
        toolchain = toolchain,
        output_file_map = {},
        supports_output_file_map = True,
        supports_serialized_errors = False,
        skip_incremental_outputs = True,
        incremental_remote_outputs = False,
        objects = [],
        incremental_artifacts = None,
        artifact_tag = None,
    )

    typecheck_file = ctx.actions.declare_output("swift-typecheck-stderr", has_content_based_path = uses_content_based_paths)

    cxx_toolchain = get_cxx_toolchain_info(ctx)
    typecheck_cmd = cmd_args([
        cxx_toolchain.internal_tools.stderr_to_file,
        cmd_args(typecheck_file.as_output(), format = "--out={}"),
        swift_cmd_output.cmd,
    ])

    ctx.actions.run(
        typecheck_cmd,
        category = category,
    )

    return typecheck_file

def _compile_object(
    ctx: AnalysisContext,
    toolchain: SwiftToolchainInfo,
    shared_flags: cmd_args,
    srcs: list[CxxSrcWithFlags],
    output_swiftmodule: Artifact,
    output_header: Artifact,
    inputs_tag: ArtifactTag,
    category: str,
) -> SwiftObjectOutput:
    dep_files = {}
    output_file_map = {}
    emit_depsfiles = toolchain.use_depsfiles and not get_incremental_file_hashing_enabled(ctx) and not get_incremental_split_actions(ctx)
    skip_incremental_outputs = False
    module_name = get_module_name(ctx)
    uses_content_based_paths = get_uses_content_based_paths(ctx)

    if should_build_swift_incrementally(ctx):
        output_swiftdoc = None
        if not get_incremental_split_actions(ctx):
            # define this here as we will only have an artifact for the purposes of incremental rebuilds.
            output_swiftdoc = ctx.actions.declare_output(module_name + ".swiftdoc", has_content_based_path = uses_content_based_paths)

        incremental_compilation_output = get_incremental_object_compilation_flags(ctx, srcs, output_swiftmodule, output_swiftdoc, output_header)
        cmd = incremental_compilation_output.incremental_flags_cmd

        # With -skip-incremental-output the output_file_map is an output, so
        # we cannot support depsfiles.
        emit_depsfiles = emit_depsfiles and not incremental_compilation_output.skip_incremental_outputs
        num_threads = incremental_compilation_output.num_threads
        objects = incremental_compilation_output.artifacts
        output_file_map = incremental_compilation_output.output_file_map
        skip_incremental_outputs = incremental_compilation_output.skip_incremental_outputs
        swiftdeps = incremental_compilation_output.swiftdeps
        depfiles = incremental_compilation_output.depfiles
        incremental_artifacts = IncrementalCompilationInput(depfiles = depfiles, swiftdeps = swiftdeps, swiftdoc = output_swiftdoc)
    else:
        num_threads = 1
        swiftdeps = []
        output_object = ctx.actions.declare_output(module_name + SWIFT_EXTENSION + ".o", has_content_based_path = uses_content_based_paths)
        objects = [output_object]
        object_format = toolchain.object_format.value
        embed_bitcode = False
        if toolchain.object_format == SwiftObjectFormat("object-embed-bitcode"):
            object_format = "object"
            embed_bitcode = True

        cmd = cmd_args([
            "-emit-{}".format(object_format),
            "-o",
            output_object.as_output(),
            "-wmo",
        ])
        incremental_artifacts = None

        if embed_bitcode:
            cmd.add("--embed-bitcode")

    if emit_depsfiles:
        add_dependencies_output(ctx, output_file_map, cmd, "object", inputs_tag)
        dep_files["object"] = inputs_tag

    if _should_compile_with_evolution(ctx):
        cmd.add(["-enable-library-evolution"])

    argsfiles, output_map_artifact = _compile_with_argsfile(
        ctx = ctx,
        category = category,
        extension = SWIFT_EXTENSION,
        shared_flags = shared_flags,
        srcs = srcs,
        additional_flags = cmd,
        toolchain = toolchain,
        num_threads = num_threads,
        dep_files = dep_files,
        output_file_map = output_file_map,
        skip_incremental_outputs = skip_incremental_outputs,
        objects = objects,
        incremental_artifacts = incremental_artifacts,
        artifact_tag = inputs_tag,
    )

    return SwiftObjectOutput(
        object_files = objects,
        argsfiles = argsfiles,
        output_map_artifact = output_map_artifact,
        swiftdeps = swiftdeps,
    )

def _compile_modularization_dependency_graph(
    ctx: AnalysisContext, toolchain: SwiftToolchainInfo, shared_flags: cmd_args, srcs: list[CxxSrcWithFlags]
) -> Artifact | None:
    # -emit-modularization-dependency-dot-graph is only understood by the Pika
    # toolchain's swiftc; Xcode's fails the action with "unknown argument".
    if not toolchain.supports_modularization_dependency_graph:
        return None

    modularization_dependecy_graph_output = ctx.actions.declare_output(
        "__modularization_dependency_graph__/swift_{}.dot".format(get_module_name(ctx)), has_content_based_path = False
    )
    cmd = cmd_args(
        "-emit-modularization-dependency-dot-graph",
        modularization_dependecy_graph_output.as_output(),
        "-emit-object",
        "-enable-batch-mode",
        "-driver-batch-size-limit",
        str(INCREMENTAL_SWIFT_COMPILE_BATCH_SIZE),
        "-j",
        str(INCREMENTAL_SWIFT_COMPILE_MAX_NUM_THREADS),
    )

    category = "modularization_dependency_graph_compile"

    output_file_map, output_objects, output_modularization_dependency_graph_shards = get_modularization_dependency_graph_output_map(ctx, srcs)
    for object in output_objects:
        cmd.add(cmd_args(hidden = object.as_output()))

    for shard in output_modularization_dependency_graph_shards:
        cmd.add(cmd_args(hidden = shard.as_output()))

    _compile_with_argsfile(
        ctx = ctx,
        category = category,
        extension = SWIFT_EXTENSION,
        shared_flags = shared_flags,
        srcs = srcs,
        additional_flags = cmd,
        toolchain = toolchain,
        num_threads = INCREMENTAL_SWIFT_COMPILE_MAX_NUM_THREADS,
        output_file_map = output_file_map,
        skip_incremental_outputs = False,
        objects = output_objects,
    )
    return modularization_dependecy_graph_output

def _compile_index_store(ctx: AnalysisContext, toolchain: SwiftToolchainInfo, shared_flags: cmd_args, srcs: list[CxxSrcWithFlags]) -> Artifact:
    module_name = get_module_name(ctx)

    # Create a directory where we can write .o files and ignore them.
    #
    # In index store directories, the hash used in the unit file name is derived from
    # the output path. E.g. if the output path is dir1/Foo.o, our unit file is
    # Foo.swift-ABC123. If the output path changes to dir2/Foo.o, our unit file is now
    # Foo.swift-DEF456.
    #
    # See this part of the index-while-building talk from the 2017 LLVM developer
    # meeting: https://www.youtube.com/watch?v=jGJhnIT-D2M&t=279s
    #
    # swiftc also requires that all output paths are unique, so we can't just use
    # /dev/null as the output path.
    #
    # Buck requires that all declared outputs are created when the rule succeeds, but we
    # want to create an index store even if there are errors in the source files.
    #
    # We solve this by declaring a directory as the buck output, and telling swiftc that
    # we want all our .o files there. Our target will then succeed whether or not we
    # create a .o file.
    objects_dir = ctx.actions.declare_output("__indexstore__/objects", dir = True, has_content_based_path = False)

    sh_cmd = cmd_args([
        "mkdir",
        "-p",
        objects_dir.as_output(),
    ])
    ctx.actions.run(
        sh_cmd,
        category = "swift_compilation_database",
    )

    # We need an output file map with index-unit-output-path entries to be able
    # to index all of the srcs in a single pass.
    output_file_map = {}
    for src in srcs:
        output_file_map[src.file] = {
            "diagnostics": cmd_args(objects_dir, format = "{}/" + src.file.basename + ".dia", delimiter = ""),
            # The output here is only used for the identifier of the index unit file
            "index-unit-output-path": src.file,
            "object": cmd_args(objects_dir, format = "{}/" + src.file.basename + ".o", delimiter = ""),
        }

    index_store_output = ctx.actions.declare_output("__indexstore__/swift_{}".format(module_name), dir = True, has_content_based_path = False)
    additional_flags = cmd_args(
        ([] if _INDEX_SYSTEM_MODULES else ["-index-ignore-system-modules"])
        + [
            "-index-store-path",
            index_store_output.as_output(),
            "-c",
            "-disable-batch-mode",
            "-Xwrapper",
            "-ignore-errors",
        ]
    )

    _compile_with_argsfile(
        ctx = ctx,
        category = "swift_index_compile",
        shared_flags = shared_flags,
        srcs = srcs,
        additional_flags = additional_flags,
        toolchain = toolchain,
        cacheable = True,
        # Incremental builds are never cached, but caching the index store build is very useful
        # when building all the transitive index stores (e.g. during the Glean indexer).
        incremental_build_allowed = False,
        output_file_map = output_file_map,
    )

    return index_store_output

def _compile_with_argsfile(
    ctx: AnalysisContext,
    category: str,
    shared_flags: cmd_args,
    srcs: list[CxxSrcWithFlags],
    additional_flags: cmd_args,
    toolchain: SwiftToolchainInfo,
    extension: str | None = None,
    num_threads: int = 1,
    dep_files: dict[str, ArtifactTag] = {},
    output_file_map: dict = {},
    cacheable = True,
    skip_incremental_outputs = False,
    supports_serialized_errors = True,
    incremental_build_allowed = True,
    objects = [],
    incremental_artifacts: IncrementalCompilationInput | None = None,
    artifact_tag: ArtifactTag | None = None,
) -> (CompileArgsfiles | None, Artifact | None):
    build_swift_incrementally = incremental_build_allowed and should_build_swift_incrementally(ctx)

    explicit_modules_enabled = uses_explicit_modules(ctx)

    # The main compilation actions add a category suffix when explicit modules
    # are enabled for debugging and telemetry reasons. This is very important,
    # so do not be tempted to remove the suffixing.
    if extension and explicit_modules_enabled:
        category += EXPLICIT_MODULES_CATEGORY_SUFFIX

    allow_cache_upload, action_execution_attributes = _get_action_properties(toolchain, cacheable, build_swift_incrementally, explicit_modules_enabled)

    argsfile, output_file_map = compile_with_argsfile(
        ctx = ctx,
        category = category,
        shared_flags = shared_flags,
        srcs = srcs,
        additional_flags = additional_flags,
        toolchain = toolchain,
        num_threads = num_threads,
        dep_files = dep_files,
        output_file_map = output_file_map,
        allow_cache_upload = allow_cache_upload,
        local_only = action_execution_attributes.local_only,
        prefer_local = action_execution_attributes.prefer_local,
        # We need to preserve the action outputs for incremental compilation.
        no_outputs_cleanup = build_swift_incrementally,
        # Skip incremental outputs requires an empty output file map, so is not
        # compatible with serialized diagnostics.
        supports_serialized_errors = (not skip_incremental_outputs) and supports_serialized_errors,
        skip_incremental_outputs = skip_incremental_outputs,
        objects = objects,
        incremental_artifacts = incremental_artifacts,
        artifact_tag = artifact_tag,
        incremental_remote_outputs = get_incremental_remote_outputs_enabled(ctx) and build_swift_incrementally,
    )

    if extension:
        # Swift correctly handles relative paths and we can utilize the relative argsfile for Xcode.
        return CompileArgsfiles(relative = {extension: argsfile}, xcode = {extension: argsfile}), output_file_map
    else:
        return None, output_file_map

def _get_action_properties(
    toolchain: SwiftToolchainInfo, cacheable: bool, build_swift_incrementally: bool, explicit_modules_enabled: bool
) -> (bool, ActionExecutionAttributes):  # (allow_cache_upload, ActionExecutionAttributes)
    # By default, we allow Buck and any command line arguments to determine execution preference.
    # However, based upon certain functionality or execution environments, we modify these
    # default properties for cacheability and performance.

    # If we prefer to execute locally (e.g., for perf reasons), ensure we upload to the cache so
    # that CI builds populate caches used by developer machines.
    allow_cache_upload = True

    local_only = False

    # Swift compilation on RE without explicit modules is impractically expensive because there's
    # no shared module cache across different libraries.
    prefer_local = not explicit_modules_enabled

    if build_swift_incrementally:
        # When Swift code is built incrementally, the swift-driver embeds absolute paths into the
        # artifacts without relative resource dir support. In this case we can only build locally.
        if not toolchain.supports_relative_resource_dir:
            allow_cache_upload = False
            local_only = True
            prefer_local = False

    if not cacheable:
        allow_cache_upload = False
        local_only = True
        prefer_local = False

    return (allow_cache_upload, ActionExecutionAttributes(prefer_local = prefer_local, local_only = local_only))

def _get_serialize_debugging_options(ctx: AnalysisContext, uses_explicit_modules: bool):
    if ctx.attrs.serialize_debugging_options == False:
        return False

    if uses_explicit_modules:
        # If the toolchain supports explicit modules we can
        # enable, regardless if this is a mixed library or not
        return get_swift_toolchain_info(ctx).supports_explicit_module_debug_serialization

    # Serialized debugging options contain absolute paths until
    # https://github.com/swiftlang/swift/pull/84112
    return False

def _get_shared_flags(
    ctx: AnalysisContext,
    deps_providers: list,
    parse_as_library: bool,
    public_module: SwiftCompiledModuleInfo | None,
    module_name: str,
    public_modulemap_pp_info: CPreprocessor | None,
    extra_search_paths_flags: list[ArgLike],
    inputs_tag: ArtifactTag,
    is_macro: bool,
    thunk_of_module: SwiftCompiledModuleInfo | None = None,
) -> (cmd_args, Artifact | None):
    toolchain = get_swift_toolchain_info(ctx)
    cmd = cmd_args()

    if not toolchain.supports_relative_resource_dir:
        # Setting this to empty will get the driver to make all paths absolute when
        # passed to the frontend. We later debug prefix these to ensure relative paths
        # in the debug info.
        cmd.add(["-working-directory="])

    cmd.add(get_sdk_flags(ctx))
    cmd.add(_get_target_flags(ctx))
    cmd.add(
        "-swift-version",
        ctx.attrs.swift_version,
        # Always use color, consistent with clang.
        "-color-diagnostics",
        # Unset the working directory in the debug information.
        "-file-compilation-dir",
        ".",
        "-module-name",
        module_name,
        "-Xfrontend",
        "-enable-cross-import-overlays",
        "-Xfrontend",
        "-disable-cxx-interop-requirement-at-import",
        "-Xfrontend",
        "-emit-clang-header-nonmodular-includes",
        # RE actions are run in a sandbox, which will fail to run macros as
        # you cannot nest sandbox actions.
        # https://github.com/swiftlang/swift/pull/70079
        "-disable-sandbox",
    )

    if parse_as_library:
        cmd.add([
            "-parse-as-library",
        ])

    if ctx.attrs.swift_package_name != None:
        cmd.add([
            "-package-name",
            ctx.attrs.swift_package_name,
        ])

    explicit_modules_enabled = uses_explicit_modules(ctx)
    if explicit_modules_enabled:
        cmd.add([
            "-Xcc",
            "-Xclang",
            "-Xcc",
            # We set -fmodule-file-home-is-cwd as this is used to correctly
            # set the working directory of modules when generating debug info.
            "-fmodule-file-home-is-cwd",
            "-Xcc",
            "-Xclang",
            "-Xcc",
            # This is the default for compilation, but not in sourcekitd.
            # Set it explicitly here so that indexing will not fail with
            # invalid module format errors.
            "-fmodule-format=obj",
        ])
        cmd.add(get_disable_pch_validation_flags())
    else:
        # Clang module breadcrumbs include module hashes and temporary paths
        # and need to be excluded for deterministic build output.
        cmd.add("-Xfrontend", "-no-clang-module-breadcrumbs")

    if toolchain.resource_dir:
        cmd.add([
            "-resource-dir",
            toolchain.resource_dir,
        ])

    if ctx.attrs.enable_cxx_interop:
        cmd.add(["-cxx-interoperability-mode=default"])

    if _get_serialize_debugging_options(ctx, explicit_modules_enabled):
        cmd.add([
            "-Xfrontend",
            "-serialize-debugging-options",
            "-Xfrontend",
            "-prefix-serialized-debugging-options",
        ])
    else:
        cmd.add([
            "-Xfrontend",
            "-no-serialize-debugging-options",
        ])

    upcoming_features = toolchain.swift_upcoming_features[ctx.attrs.swift_version]
    experimental_features = toolchain.swift_experimental_features[ctx.attrs.swift_version]

    for feature in upcoming_features:
        cmd.add([
            "-Xfrontend",
            "-enable-upcoming-feature",
            "-Xfrontend",
            feature,
        ])

    for feature in experimental_features:
        cmd.add([
            "-Xfrontend",
            "-enable-experimental-feature",
            "-Xfrontend",
            feature,
        ])

    if ctx.attrs._swift_enable_testing:
        cmd.add("-enable-testing")

    if uses_dynamic_replacement(ctx) and not is_macro:
        cmd.add(_DYNAMIC_REPLACEMENT_FLAGS)

    if getattr(ctx.attrs, "application_extension", False):
        cmd.add("-application-extension")

    # Only apple_library and apple_test has swift_macro_deps
    swift_macros = getattr(ctx.attrs, "swift_macro_deps", [])
    if swift_macros:
        for m in ctx.plugins[SwiftMacroPlugin]:
            macro_artifact = m[DefaultInfo].default_outputs[0]
            cmd.add([
                "-load-plugin-executable",
                cmd_args(inputs_tag.tag_artifacts(macro_artifact), format = "{}#" + macro_artifact.basename),
            ])

    pcm_deps_tset = get_compiled_pcm_deps_tset(ctx, deps_providers)

    # If Swift Explicit Modules are enabled, a few things must be provided to a compilation job:
    # 1. Direct and transitive SDK deps from `sdk_modules` attribute.
    # 2. Direct and transitive user-defined deps.
    # 3. Transitive SDK deps of user-defined deps.
    # (This is the case, when a user-defined dep exports a type from SDK module,
    # thus such SDK module should be implicitly visible to consumers of that custom dep)
    swift_module_map_artifact = None
    if uses_explicit_modules(ctx):
        sdk_clang_deps_tset = get_compiled_sdk_clang_deps_tset(ctx, deps_providers)
        sdk_swift_deps_tset = get_compiled_sdk_swift_deps_tset(ctx, deps_providers)
        swift_module_map_artifact = _add_swift_module_map_args(
            ctx = ctx,
            sdk_swiftmodule_deps_tset = sdk_swift_deps_tset,
            pcm_deps_tset = pcm_deps_tset,
            sdk_deps_tset = sdk_clang_deps_tset,
            inputs_tag = inputs_tag,
            cmd = cmd,
            is_macro = is_macro,
            map_module_name = module_name,
            own_module = thunk_of_module,
        )

    _add_clang_deps_flags(ctx, pcm_deps_tset, cmd, inputs_tag)
    _add_swift_deps_flags(ctx, cmd, is_macro, own_module = thunk_of_module)

    # Add flags for importing the ObjC part of this library
    _add_mixed_library_flags_to_cmd(
        ctx,
        cmd,
        public_module,
        public_modulemap_pp_info,
        import_underlying_module = thunk_of_module == None,
    )

    # Add toolchain and target flags last to allow for overriding defaults
    cmd.add(toolchain.compiler_flags)
    cmd.add(ctx.attrs.swift_compiler_flags)
    cmd.add(extra_search_paths_flags)

    return (cmd, swift_module_map_artifact)

def _add_swift_module_map_args(
    ctx: AnalysisContext,
    sdk_swiftmodule_deps_tset: SwiftCompiledModuleTset,
    pcm_deps_tset: SwiftCompiledModuleTset,
    sdk_deps_tset: SwiftCompiledModuleTset,
    inputs_tag: ArtifactTag,
    cmd: cmd_args,
    is_macro: bool,
    map_module_name: str,
    own_module: SwiftCompiledModuleInfo | None = None,
) -> Artifact:
    sdk_swiftmodule_deps_tset = [sdk_swiftmodule_deps_tset] if sdk_swiftmodule_deps_tset else []
    children = (
        _get_swift_paths_tsets(is_macro, ctx.attrs.deps + getattr(ctx.attrs, "exported_deps", [])) + [pcm_deps_tset, sdk_deps_tset] + sdk_swiftmodule_deps_tset
    )
    if own_module:
        all_deps_tset = ctx.actions.tset(SwiftCompiledModuleTset, value = own_module, children = children)
    else:
        all_deps_tset = ctx.actions.tset(SwiftCompiledModuleTset, children = children)
    swift_module_map_artifact, swift_module_map_args = write_swift_module_map_with_deps(
        ctx,
        map_module_name,
        all_deps_tset,
    )
    cmd.add([
        "-Xfrontend",
        "-explicit-swift-module-map-file",
        "-Xfrontend",
        inputs_tag.tag_artifacts(swift_module_map_args),
    ])
    return swift_module_map_artifact

def _add_swift_deps_flags(
    ctx: AnalysisContext,
    cmd: cmd_args,
    is_macro: bool,
    own_module: SwiftCompiledModuleInfo | None = None,
):
    if uses_explicit_modules(ctx):
        cmd.add([
            "-Xcc",
            "-fno-implicit-modules",
            "-Xcc",
            "-fno-implicit-module-maps",
            "-Xfrontend",
            "-disable-implicit-swift-modules",
        ])
    else:
        children = _get_swift_paths_tsets(is_macro, ctx.attrs.deps + getattr(ctx.attrs, "exported_deps", []))
        if own_module:
            depset = ctx.actions.tset(SwiftCompiledModuleTset, value = own_module, children = children)
        else:
            depset = ctx.actions.tset(SwiftCompiledModuleTset, children = children)
        cmd.add(depset.project_as_args("module_search_path"))

        implicit_search_path_tset = get_implicit_framework_search_path_providers(
            ctx,
            None,
            ctx.attrs.deps + getattr(ctx.attrs, "exported_deps", []),
        )
        cmd.add(implicit_search_path_tset.project_as_args("swift_framework_implicit_search_paths_args"))

def _add_clang_deps_flags(ctx: AnalysisContext, pcm_deps_tset: SwiftCompiledModuleTset, cmd: cmd_args, inputs_tag: ArtifactTag) -> None:
    if uses_explicit_modules(ctx):
        clang_flags = pcm_deps_tset.project_as_args("clang_importer_flags")
        cmd.add(inputs_tag.tag_artifacts(clang_flags))
    else:
        inherited_preprocessor_infos = cxx_inherited_preprocessor_infos(ctx.attrs.deps + getattr(ctx.attrs, "exported_deps", []))
        preprocessors = cxx_merge_cpreprocessors(ctx.actions, [], inherited_preprocessor_infos)
        cmd.add(cmd_args(preprocessors.set.project_as_args("args"), prepend = "-Xcc"))
        cmd.add(cmd_args(preprocessors.set.project_as_args("modular_args"), prepend = "-Xcc"))

def _add_mixed_library_flags_to_cmd(
    ctx: AnalysisContext,
    cmd: cmd_args,
    underlying_module: SwiftCompiledModuleInfo | None,
    public_modulemap_pp_info: CPreprocessor | None,
    import_underlying_module: bool = True,
) -> None:
    if uses_explicit_modules(ctx):
        if underlying_module:
            cmd.add(underlying_module.clang_importer_args)
            cmd.add(clang_module_file_args(underlying_module))
            if import_underlying_module:
                cmd.add("-import-underlying-module")
        return

    for objc_modulemap_pp_info in filter(None, [public_modulemap_pp_info]):
        # TODO(T99100029): We cannot use VFS overlays to mask this import from
        # the debugger as they require absolute paths. Instead we will enforce
        # that mixed libraries do not have serialized debugging info and rely on
        # rdeps to serialize the correct paths.
        for arg in objc_modulemap_pp_info.args.args:
            cmd.add("-Xcc")
            cmd.add(arg)

        for arg in objc_modulemap_pp_info.modular_args:
            cmd.add("-Xcc")
            cmd.add(arg)

    if public_modulemap_pp_info and import_underlying_module:
        cmd.add("-import-underlying-module")

def _get_swift_dependency_info(is_macro: bool, deps: list[Dependency]) -> list[SwiftDependencyInfo]:
    ret = []
    for d in deps:
        if SwiftDependencyInfo in d:
            if d[SwiftDependencyInfo].is_macro != is_macro:
                fail(
                    "Cannot have a {} dep in a {} target".format(
                        "macro" if d[SwiftDependencyInfo].is_macro else "non-macro",
                        "macro" if is_macro else "non-macro",
                    )
                )
            ret.append(d[SwiftDependencyInfo])

    return ret

def _get_swift_paths_tsets(is_macro: bool, deps: list[Dependency]) -> list[SwiftCompiledModuleTset]:
    return [d.exported_swiftmodules for d in _get_swift_dependency_info(is_macro, deps)]

def get_external_debug_info_tsets(is_macro: bool, deps: list[Dependency]) -> list[ArtifactTSet]:
    return [d.debug_info_tset for d in _get_swift_dependency_info(is_macro, deps)]

def get_external_swiftmodule_change_analysis_tsets(is_macro: bool, deps: list[Dependency]) -> list[ArtifactTSet]:
    return [d.swiftmodule_change_analysis_tset for d in _get_swift_dependency_info(is_macro, deps)]

def get_external_swift_ast_dump_tsets(is_macro: bool, deps: list[Dependency]) -> list[ArtifactTSet]:
    return [d.swift_ast_dump_tset for d in _get_swift_dependency_info(is_macro, deps)]

def get_swift_pcm_uncompile_info(
    ctx: AnalysisContext, propagated_exported_preprocessor_info: [CPreprocessorInfo, None], exported_pre: [CPreprocessor, None]
) -> [SwiftPCMUncompiledInfo, None]:
    swift_toolchain = get_swift_toolchain_info(ctx)

    if is_sdk_modules_provided(swift_toolchain):
        clang_importer_args = cmd_args()
        if propagated_exported_preprocessor_info:
            # We need to include exported preprocessor from exported_deps to
            # handle misconfigured targets that have non-modular exported_deps
            clang_importer_args.add(cmd_args(propagated_exported_preprocessor_info.set.project_as_args("args"), prepend = "-Xcc"))

        exported_clang_importer_args = cmd_args()
        if exported_pre:
            if supports_modulemaps_with_hmaps(ctx):
                # The cxx preprocessor info will not include the modular args,
                # so include the required modulemap flags for this target only.
                # The deps modular args will be included via projections on the
                # deps SwiftCompiledModuleTsets.
                clang_importer_args.add(cmd_args(exported_pre.modular_args, prepend = "-Xcc"))
            else:
                # We need special handling for our own exported_pre, which
                # contains the header search path flags for the modules header
                # symlink tree. This needs to be propagated to rdeps.
                exported_clang_importer_args.add(cmd_args(exported_pre.modular_args, prepend = "-Xcc"))

        return SwiftPCMUncompiledInfo(
            clang_importer_args = clang_importer_args,
            exported_clang_importer_args = exported_clang_importer_args,
            exported_deps = _exported_deps(ctx),
            is_transient = not ctx.attrs.modular or not exported_pre,
            modulemap_artifact = exported_pre.modulemap_artifact if exported_pre else None,
            name = get_module_name(ctx),
            uncompiled_sdk_modules = ctx.attrs.sdk_modules,
        )
    return None

def create_swift_dependency_info(
    ctx: AnalysisContext,
    deps,
    deps_providers: list,
    compiled_info: [SwiftCompiledModuleInfo, None],
    debug_info_tset: ArtifactTSet,
    swiftmodule_change_analysis_tset: ArtifactTSet,
    swift_ast_dump_tset: ArtifactTSet,
    is_macro: bool,
):
    # We pass through the SDK swiftmodules here to match Buck 1 behaviour. This is
    # pretty loose, but it matches Buck 1 behavior so cannot be improved until
    # migration is complete.
    transitive_swiftmodule_deps = _get_swift_paths_tsets(is_macro, deps) + [get_compiled_sdk_swift_deps_tset(ctx, deps_providers)]

    if compiled_info:
        exported_swiftmodules = ctx.actions.tset(SwiftCompiledModuleTset, value = compiled_info, children = transitive_swiftmodule_deps)
    else:
        exported_swiftmodules = ctx.actions.tset(SwiftCompiledModuleTset, children = transitive_swiftmodule_deps)

    return SwiftDependencyInfo(
        debug_info_tset = debug_info_tset,
        exported_swiftmodules = exported_swiftmodules,
        # prebuilt_apple_framework do not have exported_headers, we can exclude
        # them from validation safely.
        has_exported_headers = len(getattr(ctx.attrs, "exported_headers", [])) > 0,
        is_modular = ctx.attrs.modular,
        is_macro = is_macro,
        swift_ast_dump_tset = swift_ast_dump_tset,
        swiftmodule_change_analysis_tset = swiftmodule_change_analysis_tset,
    )

def get_swift_dependency_info(
    ctx: AnalysisContext,
    output_module: Artifact | None,
    deps_providers: list,
    is_macro: bool,
    swift_ast_dump_artifacts: list[Artifact] = [],
) -> SwiftDependencyInfo:
    exported_deps = _exported_deps(ctx)

    if output_module:
        compiled_info = SwiftCompiledModuleInfo(
            is_framework = False,
            is_sdk_module = False,
            is_swiftmodule = True,
            module_name = get_module_name(ctx),
            output_artifact = output_module,
        )
    else:
        compiled_info = None

    all_deps = ctx.attrs.deps + getattr(ctx.attrs, "exported_deps", [])

    debug_info_tset = make_artifact_tset(
        actions = ctx.actions,
        artifacts = filter(None, [output_module]),
        children = get_external_debug_info_tsets(is_macro, all_deps),
        label = ctx.label,
        tags = [ArtifactInfoTag("swift_debug_info")],
    )

    # Deliberately broader than `all_deps` above (also covers toolchain
    # `default_deps` and `deps_query`, via `cxx_attr_deps`/`cxx_attr_exported_deps`):
    # this tset is new, so widening it to match the rest of the codebase's
    # convention has no existing behavior to preserve. `debug_info_tset` and
    # `swift_ast_dump_tset` predate this and keep the narrower `all_deps` to
    # avoid changing their already-shipped output.
    swiftmodule_change_analysis_deps = cxx_attr_deps(ctx) + cxx_attr_exported_deps(ctx)

    swiftmodule_change_analysis_tset = make_artifact_tset(
        actions = ctx.actions,
        artifacts = filter(None, [output_module]),
        children = get_external_swiftmodule_change_analysis_tsets(is_macro, swiftmodule_change_analysis_deps),
        label = ctx.label,
    )

    swift_ast_dump_tset = make_artifact_tset(
        actions = ctx.actions,
        artifacts = swift_ast_dump_artifacts,
        children = get_external_swift_ast_dump_tsets(is_macro, all_deps),
        label = ctx.label,
    )

    return create_swift_dependency_info(
        ctx,
        exported_deps,
        deps_providers,
        compiled_info,
        debug_info_tset,
        swiftmodule_change_analysis_tset,
        swift_ast_dump_tset,
        is_macro,
    )

def get_swiftmodule_linkable(swift_compile_output: [SwiftCompilationOutput, None]) -> [SwiftmoduleLinkable, None]:
    return SwiftmoduleLinkable(swiftmodules = swift_compile_output.swift_debug_info) if swift_compile_output else None

def extract_swiftmodule_linkables(link_infos: [list[LinkInfo], None]) -> list[SwiftmoduleLinkable]:
    linkables = []
    for info in link_infos:
        for linkable in info.linkables:
            if isinstance(linkable, SwiftmoduleLinkable):
                linkables.append(linkable)

    return linkables

def get_swiftmodule_linker_flags(ctx: AnalysisContext, swiftmodule_linkable: [SwiftmoduleLinkable, None]) -> cmd_args:
    if swiftmodule_linkable:
        tset = swiftmodule_linkable.swiftmodules
        artifacts = project_artifacts(
            actions = ctx.actions,
            tsets = tset,
        )
        return cmd_args([cmd_args(swiftmodule, format = "-Wl,-add_ast_path,{}") for swiftmodule in artifacts])
    return cmd_args()

def _get_xctest_swiftmodule_search_path(ctx: AnalysisContext) -> cmd_args:
    # With explicit modules we don't need to search at all.
    if uses_explicit_modules(ctx):
        return cmd_args()

    for fw in ctx.attrs.frameworks:
        if fw.endswith("XCTest.framework"):
            toolchain = ctx.attrs._apple_toolchain[AppleToolchainInfo]
            return cmd_args(toolchain.platform_path, format = "-I{}/Developer/usr/lib")

    return cmd_args()

def get_swift_debug_infos(
    ctx: AnalysisContext, swift_dependency_info: [SwiftDependencyInfo, None], swift_output: [SwiftCompilationOutput, None]
) -> SwiftDebugInfo:
    # When determining the debug info for shared libraries, if the shared library is a link group, we rely on the link group links to
    # obtain the debug info for linked libraries and only need to provide any swift debug info for this library itself. Otherwise
    # if linking standard shared, we need to obtain the transitive debug info.
    if get_link_group(ctx):
        swift_shared_debug_info = []
    else:
        swift_shared_debug_info = _get_swift_shared_debug_info(swift_dependency_info) if swift_dependency_info else []

    clang_debug_info = [swift_output.clang_debug_info] if swift_output else []
    swift_debug_info = [swift_output.swift_debug_info] if swift_output else []

    return SwiftDebugInfo(
        static = clang_debug_info + swift_debug_info,
        shared = swift_shared_debug_info + clang_debug_info + swift_debug_info,
    )

def _get_swift_shared_debug_info(swift_dependency_info: SwiftDependencyInfo) -> list[ArtifactTSet]:
    return [swift_dependency_info.debug_info_tset] if swift_dependency_info.debug_info_tset else []

def _get_project_root_file(ctx) -> Artifact:
    content = cmd_args(ctx.label.project_root)
    return ctx.actions.write("project_root_file", content, absolute = True, has_content_based_path = False)

def _create_compilation_database(ctx: AnalysisContext, srcs: list[CxxSrcWithFlags], argfile: CompileArgsfile) -> SwiftCompilationDatabase:
    module_name = get_module_name(ctx)

    swift_toolchain = get_swift_toolchain_info(ctx)
    mk_comp_db = swift_toolchain.mk_swift_comp_db

    identifier = module_name + ".swift_comp_db.json"
    cdb_artifact = ctx.actions.declare_output(identifier, has_content_based_path = False)

    srcs_args = cmd_args([s.file for s in srcs])

    cmd = cmd_args(mk_comp_db)
    cmd.add(cmd_args(cdb_artifact.as_output(), format = "--output={}"))
    cmd.add(cmd_args(_get_project_root_file(ctx), format = "--project-root-file={}"))
    cmd.add("--files", srcs_args)

    cmd.add("--")

    cmd_form_and_srcs = cmd_args(argfile.cmd_form, srcs_args)
    cmd.add(cmd_form_and_srcs)

    ctx.actions.run(
        cmd,
        category = "swift_compilation_database",
        identifier = identifier,
        error_handler = apple_build_error_handler,
    )

    return SwiftCompilationDatabase(db = cdb_artifact, other_outputs = cmd_form_and_srcs)

def _create_objc_swift_interface(ctx: AnalysisContext, shared_flags: cmd_args, module_name: str) -> DefaultInfo:
    """Generates the Swift interface representation of a modular Obj-C(++) target."""
    swift_toolchain = get_swift_toolchain_info(ctx)
    swift_ide_test_tool = swift_toolchain.swift_ide_test_tool
    if not swift_ide_test_tool:
        return DefaultInfo()
    mk_swift_interface = swift_toolchain.mk_swift_interface
    uses_content_based_paths = get_uses_content_based_paths(ctx)

    identifier = module_name + ".swift_interface"

    argsfile, _ = ctx.actions.write(
        identifier + "_argsfile",
        shared_flags,
        allow_args = True,
        has_content_based_path = uses_content_based_paths,
    )
    interface_artifact = ctx.actions.declare_output(identifier, has_content_based_path = uses_content_based_paths)

    mk_swift_args = cmd_args(
        mk_swift_interface,
        "--swift-ide-test-tool",
        swift_ide_test_tool,
        "--module",
        module_name,
        "--out",
        interface_artifact.as_output(),
        "--",
        cmd_args(cmd_args(argsfile, format = "@{}", delimiter = ""), hidden = [shared_flags]),
    )

    ctx.actions.run(
        mk_swift_args,
        category = "mk_swift_interface",
        identifier = identifier,
        error_handler = apple_build_error_handler,
    )

    return DefaultInfo(
        default_output = interface_artifact,
        other_outputs = [
            argsfile,
        ],
    )

def _exported_deps(ctx) -> list[Dependency]:
    if ctx.attrs.reexport_all_header_dependencies:
        return getattr(ctx.attrs, "exported_deps", []) + ctx.attrs.deps
    else:
        return getattr(ctx.attrs, "exported_deps", [])

def _should_compile_with_evolution(ctx) -> bool:
    if ctx.attrs.enable_library_evolution != None:
        return ctx.attrs.enable_library_evolution
    return ctx.attrs._enable_library_evolution
