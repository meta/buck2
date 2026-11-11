# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load(
    "@prelude//cxx:compile_types.bzl",
    "AsmExtensions",
    "CudaDistributedCompileOutput",
    "CxxCompileCommand",
    "CxxCompileCommandOutput",
    "CxxCompileFlavor",
    "CxxCompileOutput",
    "CxxExtension",
    "CxxSrcCompileCommand",
    "CxxSrcPrecompileCommand",
    "DepFileType",
    "HeaderExtension",
    "HeadersDepFiles",
    "IndexStoreFactory",
    "UseHeaderUnitsMode",
)
load(
    "@prelude//cxx:cuda.bzl",
    "CudaCompileInfo",
    "CudaCompileStyle",  # @unused Used as a type
    "CudaDistributedCompileSpec",  # @unused Used as a type
    "create_cuda_distributed_compiles",
    "cuda_compile",
    "declare_cuda_dist_compile_output",
)
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo")
load("@prelude//cxx:hip.bzl", "hip_compile_weight")
load("@prelude//cxx:hip_debug_extract.bzl", "hip_debug_extract_available", "hip_debug_extract_compile_object")
load(
    "@prelude//ide_integrations/xcode:argsfiles.bzl",
    "XCODE_ARG_SUBSTITUTIONS",
)
load("@prelude//linking:lto.bzl", "LtoMode")
load("@prelude//utils:argfile.bzl", "argsfile_with_artifacts", _mk_argsfile = "mk_argsfile")
load("@prelude//utils:expect.bzl", "expect")
load(
    "@prelude//utils:utils.bzl",
    "as_output",
    "flatten",
    "map_val",
)
load(":argsfiles.bzl", "CompileArgsfile", "CompileArgsfiles")
load(":attr_selection.bzl", "cxx_by_language_ext")
load(
    ":compiler.bzl",
    "get_flags_for_colorful_output",
    "get_flags_for_reproducible_build",
    "get_headers_dep_files_flags_factory",
    "get_output_flags",
)
load(":cxx_context.bzl", "get_cxx_toolchain_info")
load(
    ":cxx_flags.bzl",
    "CxxFlagsInfo",
    "cxx_flags_projection_name",
    "cxx_flags_tset",
)
load(":cxx_library_utility.bzl", "EMPTY_DEFAULT_INFO")
load(":cxx_sources.bzl", "CxxSrcWithFlags")
load(":cxx_toolchain_types.bzl", "CxxObjectFormat", "DepTrackingMode", "compiler_info_with_argsfiles")
load(":cxx_types.bzl", "CxxRuleConstructorParams")
load(":debug.bzl", "SplitDebugMode")
load(
    ":headers.bzl",
    "CHeader",
    "CPrecompiledHeaderInfo",
    "CxxPrecompiledHeader",  # @unused Used in type annotation
    "add_headers_dep_files",
)
load(
    ":preprocessor.bzl",
    "CPreprocessor",  # @unused Used as a type
    "CPreprocessorInfo",  # @unused Used as a type
    "HeaderUnit",  # @unused Used as a type
    "cxx_merge_cpreprocessors",
    "get_flags_for_compiler_type",
)

# Output folder shared by every per-source object declaration; the shared CUDA
# plan must declare its outputs under the same folder as the objects it feeds.
_OBJECTS_FOLDER = "__objects__"

_CUDA_DRYRUN_SOURCE_PLACEHOLDER = "__buck2_cuda_dryrun_placeholder__.cu"

# Record containing compile info that will be passed to the dynamic action
CxxCompileInfo = record(
    src = field(Artifact),
    args = field(list[typing.Any]),
    short_path = field(str),
    filename_base = field(str),
    index_store_base = field(str | None),  # filename_base for index store compilation
    identifier = field(str),
    folder_name = field(str),
    weight = field(int),
)

CxxSharedCompileInfo = record(
    cxx_compile_cmd = field(CxxCompileCommand),
    index_store_factory = field(IndexStoreFactory | None),
    error_handler = field([typing.Callable, None]),
    expect_eligible_for_dedupe = field(bool),
)

# Input for a single CXX compilation - declared artifacts and compile info
CxxCompileInput = record(
    declared_artifacts = field(CxxCompileOutput),
    info = field(CxxCompileInfo),
)

CxxSharedCompileCommandArgs = record(
    before_src = field(cmd_args),
    after_src = field(cmd_args),
)

def _project_clang_traces_as_args(traces: list[Artifact]):
    return cmd_args(traces)

ClangTracesTSet = transitive_set(
    args_projections = {
        "clang_traces": _project_clang_traces_as_args,
    },
)

ClangTracesInfo = provider(
    fields = {
        "clang_traces": provider_field(typing.Any, default = None),  # "ClangTracesTSet"
    }
)
PicClangTracesInfo = provider(
    fields = {
        "clang_traces": provider_field(typing.Any, default = None),  # "ClangTracesTSet"
    }
)

def get_source_extension_for_header(header_extension: str, default: CxxExtension) -> CxxExtension:
    """
    Which source file extension to use to get compiler flags for the header.
    """
    if header_extension in (".hpp", ".hh", ".h++", ".hxx"):
        return CxxExtension(".cpp")
    elif header_extension == ".cuh":
        return CxxExtension(".cu")
    elif header_extension not in HeaderExtension.values():
        return CxxExtension(header_extension)  # a file in `headers` has a source extension
    else:
        return default

def get_source_extension(src: CxxSrcWithFlags, default_for_headers: CxxExtension) -> CxxExtension:
    """
    Which source files extension to use for a source or a header file. We want
    headers to appear as though they are source files.
    """
    if src.is_header:
        return get_source_extension_for_header(src.file.extension, default_for_headers)
    else:
        return CxxExtension(src.file.extension)

def collect_extensions(srcs: list[CxxSrcWithFlags]) -> set[CxxExtension]:
    """
    Collect extensions of source files while doing light normalization.
    """

    duplicates = {
        ".c++": ".cpp",
        ".cc": ".cpp",
        ".cxx": ".cpp",
    }

    extensions = set([CxxExtension(duplicates.get(src.file.extension, src.file.extension)) for src in srcs])
    return extensions

def default_source_extension_for_plain_header(rule_type: str) -> CxxExtension:
    """
    Returns default source file extension to use to get get compiler flags for plain .h headers.
    """

    # Default to (Objective-)C++ instead of plain (Objective-)C as it is more likely to be compatible with both.
    return CxxExtension(".mm") if rule_type.startswith("apple_") else CxxExtension(".cpp")

def detect_source_extension_for_plain_headers(exts: set[CxxExtension], rule_type: str) -> CxxExtension:
    """
    For a given list source files determine which source file extension
    to use to get compiler flags for plain .h headers.
    """

    # Assembly doesn't need any special handling as included files tend to have .asm extension themselves.
    # And the presence of assembly in the target doesn't tell us anything about the language of .h files.
    for asm_ext in AsmExtensions:
        exts.discard(asm_ext)

    if len(exts) == 0:
        return default_source_extension_for_plain_header(rule_type)

    if len(exts) == 1:
        return exts.pop()
    if CxxExtension(".hip") in exts:
        return CxxExtension(".hip")
    if CxxExtension(".cu") in exts:
        return CxxExtension(".cu")
    if CxxExtension(".mm") in exts:
        return CxxExtension(".mm")
    if CxxExtension(".cpp") in exts and CxxExtension(".m") in exts:
        return CxxExtension(".mm")
    if CxxExtension(".cpp") in exts:
        return CxxExtension(".cpp")
    if CxxExtension(".m") in exts:
        return CxxExtension(".m")
    return CxxExtension(".c")

def collect_source_extensions(srcs: list[CxxSrcWithFlags], default_for_headers: CxxExtension) -> set[CxxExtension]:
    """
    Return unique source extensions from a list of source and header files where
    header extensions are mapped to corresponding source extensions.
    """
    source_extensions = set([get_source_extension(src, default_for_headers) for src in srcs])
    return source_extensions

def get_header_language_mode(source_extension: CxxExtension) -> str | None:
    """
    Returns the header mode to use for plain .h headers based on the
    source file extension used to obtain the compiler flags for them.
    """

    # Note: CUDA doesn't have its own header language mode, but the headers have distinct .cuh extension.
    modes = {
        CxxExtension(".cpp"): "c++-header",
        CxxExtension(".m"): "objective-c-header",
        CxxExtension(".mm"): "objective-c++-header",
    }
    return modes.get(source_extension)

def create_compile_cmds(
    actions: AnalysisActions,
    target_label: Label,
    toolchain: CxxToolchainInfo,
    impl_params: CxxRuleConstructorParams,
    own_preprocessors: list[CPreprocessor],
    inherited_preprocessor_infos: list[CPreprocessorInfo],
    add_coverage_instrumentation_compiler_flags: bool,
    compile_pch: CxxPrecompiledHeader | None = None,
    filename_prefix: str = "",
) -> CxxCompileCommandOutput:
    """
    Forms the CxxSrcCompileCommand to use for each source file based on it's extension
    and optional source file flags. Returns CxxCompileCommandOutput containing an array
    of the generated compile commands and argsfile output.
    """

    if compile_pch:
        srcs_extensions = set([CxxExtension(compile_pch.clanguage)])
    else:
        srcs_extensions = collect_extensions(impl_params.srcs)
    extension_for_plain_headers = detect_source_extension_for_plain_headers(srcs_extensions, impl_params.rule_type)

    srcs_with_flags = []  # type: [CxxSrcWithFlags]

    for src in impl_params.srcs:
        srcs_with_flags.append(src)

    # Some targets have .cpp files in their `headers` lists, see D46195628
    # todo: should this be prohibited or expanded to allow all source extensions?
    artifact_extensions = HeaderExtension.values() + [".cpp"]
    all_headers = flatten([x.headers for x in own_preprocessors])
    for header in all_headers:
        if header.artifact.extension in artifact_extensions:
            srcs_with_flags.append(CxxSrcWithFlags(file = header.artifact, is_header = True))

    all_raw_headers = flatten([x.raw_headers for x in own_preprocessors])
    for header in all_raw_headers:
        if header.extension in HeaderExtension.values():
            srcs_with_flags.append(CxxSrcWithFlags(file = header, is_header = True))

    if len(srcs_with_flags) == 0:
        return CxxCompileCommandOutput()

    # TODO(T110378129): Buck v1 validates *all* headers used by a compilation
    # at compile time, but that doing that here/eagerly might be expensive (but
    # we should figure out something).
    _validate_target_headers(target_label, own_preprocessors)

    # Combine all preprocessor info and prepare it for compilations.
    pre = cxx_merge_cpreprocessors(
        actions,
        filter(None, own_preprocessors + impl_params.extra_preprocessors),
        inherited_preprocessor_infos,
    )

    headers_tag = actions.artifact_tag()

    src_compile_cmds = []
    hdr_compile_cmds = []
    cxx_compile_cmd_by_ext = {}  # type: dict[CxxExtension, CxxCompileCommand]
    argsfile_by_ext = {}  # type: dict[str, CompileArgsfile]
    xcode_argsfile_by_ext = {}  # type: dict[str, CompileArgsfile]
    if compile_pch:
        src_extensions = [CxxExtension(compile_pch.clanguage)]
    else:
        src_extensions = collect_source_extensions(srcs_with_flags, extension_for_plain_headers)

    # Deduplicate shared arguments to save memory. If we compile multiple files
    # of the same extension they will have some of the same flags. Save on
    # allocations by caching and reusing these objects.
    for ext in src_extensions:
        cmd = _generate_base_compile_command(
            actions,
            target_label,
            toolchain,
            impl_params,
            pre,
            headers_tag,
            ext,
            filename_prefix = filename_prefix,
        )
        cxx_compile_cmd_by_ext[ext] = cmd
        argsfile_by_ext[ext.value] = cmd.argsfile
        if cmd.xcode_argsfile != None:
            xcode_argsfile_by_ext[ext.value] = cmd.xcode_argsfile

    # Add -fprofile-list for selective coverage instrumentation.
    # The profile list file is generated by a clang_profile_list rule and tells
    # clang which source files to instrument, reducing overhead.
    coverage_profile_list_args = []
    if add_coverage_instrumentation_compiler_flags and impl_params.coverage_profile_list:
        coverage_profile_list_args = [cmd_args(impl_params.coverage_profile_list, format = "-fprofile-list={}")]

    for src in srcs_with_flags:
        src_args = []
        src_args.extend(src.flags)

        if compile_pch:
            ext = CxxExtension(compile_pch.clanguage)
        else:
            ext = get_source_extension(src, extension_for_plain_headers)

        cxx_compile_cmd = cxx_compile_cmd_by_ext[ext]

        if add_coverage_instrumentation_compiler_flags and cxx_compile_cmd.compiler_type != "gcc":
            src_args.extend(impl_params.coverage_instrumentation_compiler_flags)
            src_args.extend(coverage_profile_list_args)

        if src.is_header or compile_pch:
            if cxx_compile_cmd.compiler_type in ["clang", "clang_windows", "gcc"]:
                language_mode = get_header_language_mode(ext)
                src_args.extend(["-x", language_mode] if language_mode else [])
            elif cxx_compile_cmd.compiler_type in ["clang_cl", "windows", "windows_ml64"] and ext == CxxExtension(".cpp"):
                src_args.append("/TP")

        if cxx_compile_cmd.compiler_type != "nasm":
            src_args.append("-c")

        src_args.append(src.file)

        weight = 1
        if ext.value == ".hip":
            weight = hip_compile_weight(
                _target_compile_flags(impl_params, cxx_compile_cmd.compiler_type, ext) + [src.flags],
                src.file.short_path,
            )

        src_compile_command = CxxSrcCompileCommand(
            src = src.file,
            cxx_compile_cmd = cxx_compile_cmd,
            args = src_args,
            index = src.index,
            is_header = src.is_header,
            uses_content_based_paths = cxx_compile_cmd.allow_content_based_paths,
            index_store_factory = impl_params.index_store_factory,
            error_handler = impl_params.error_handler,
            expect_eligible_for_dedupe = impl_params.expect_eligible_for_dedupe,
            weight = weight,
        )

        if src.is_header:
            hdr_compile_cmds.append(src_compile_command)
        else:
            src_compile_cmds.append(src_compile_command)

    argsfile_by_ext.update(impl_params.additional.argsfiles.relative)
    xcode_argsfile_by_ext.update(impl_params.additional.argsfiles.xcode)

    return CxxCompileCommandOutput(
        src_compile_cmds = src_compile_cmds,
        base_compile_cmds = cxx_compile_cmd_by_ext,
        argsfiles = CompileArgsfiles(
            relative = argsfile_by_ext,
            xcode = xcode_argsfile_by_ext,
        ),
        comp_db_compile_cmds = src_compile_cmds + hdr_compile_cmds,
    )

COMMON_PREPROCESSOR_OUTPUT_ARGS = cmd_args("-E", "-dD")

def _prepare_cxx_compilation(
    actions: AnalysisActions,
    toolchain: CxxToolchainInfo,
    default_object_format: CxxObjectFormat,
    src_compile_cmd: CxxSrcCompileCommand,
    flavors: set[CxxCompileFlavor],
    provide_syntax_only: bool,
    use_header_units: UseHeaderUnitsMode,
    separate_debug_info: bool,
    cuda_compile_style: CudaCompileStyle | None,
    shared_cuda_dist_output: CudaDistributedCompileOutput | None,
    compile_pch: CxxPrecompiledHeader | None,
) -> CxxCompileInput:
    """
    DECLARATION PHASE: Declares all output artifacts for a single source file compilation.
    """
    short_path = src_compile_cmd.src.short_path
    if src_compile_cmd.index != None:
        # Add a unique postfix if we have duplicate source files with different flags
        short_path = short_path + "_" + str(src_compile_cmd.index)

    filename_base = short_path
    identifier = short_path

    if (
        src_compile_cmd.cxx_compile_cmd.category == "cxx_compile"
        and use_header_units == UseHeaderUnitsMode("pcm")
        and src_compile_cmd.cxx_compile_cmd.header_units_argsfile
    ):
        identifier += " (modular)"

    for flavor in flavors:
        filename_base = "{}.{}".format(filename_base, flavor.value)
        identifier = "{} ({})".format(identifier, flavor.value)

    content_based = src_compile_cmd.uses_content_based_paths
    folder_name = _OBJECTS_FOLDER
    compiler_type = src_compile_cmd.cxx_compile_cmd.compiler_type

    # Declare main object output
    object = actions.declare_output(
        folder_name,
        "{}.{}".format(filename_base, toolchain.linker_info.object_file_extension),
        has_content_based_path = content_based,
    )

    # Diagnostics
    diagnostics = None
    clang_tidy_diagnostics = None
    if compiler_type == "clang" and provide_syntax_only:
        diagnostics = actions.declare_output(
            "__diagnostics__",
            "{}.diag.txt".format(short_path),
            has_content_based_path = False,
        )
        if toolchain.binary_utilities_info.custom_tools.get("clang-tidy"):
            clang_tidy_diagnostics = actions.declare_output(
                "__diagnostics__",
                "{}.clang-tidy.txt".format(short_path),
                has_content_based_path = False,
            )

    # Declare index store upfront using factory.declare() if available
    index_store = None
    index_store_base = None
    if CxxCompileFlavor("pic") in flavors and src_compile_cmd.index_store_factory:
        factory = src_compile_cmd.index_store_factory
        declared_index_store = factory.declare(actions, src_compile_cmd)
        if declared_index_store:
            index_store = declared_index_store.output
            index_store_base = declared_index_store.filename_base

    # Clang LLVM statistics
    clang_llvm_statistics = None
    if toolchain.clang_llvm_statistics and compiler_type == "clang":
        clang_llvm_statistics = actions.declare_output(
            paths.join("__objects__", "{}.stats".format(filename_base)),
            has_content_based_path = content_based,
        )

    # GCNO file
    gcno_file = None
    if toolchain.gcno_files and src_compile_cmd.src.extension not in (".S", ".sx"):
        gcno_file = actions.declare_output(
            paths.join("__objects__", "{}.gcno".format(filename_base)),
            has_content_based_path = content_based,
        )

    # Clang remarks
    clang_remarks = None
    if toolchain.clang_remarks and compiler_type == "clang":
        clang_remarks = actions.declare_output(
            paths.join("__objects__", "{}.opt.yaml".format(filename_base)),
            has_content_based_path = content_based,
        )

    # Generate pre-processed sources
    preproc = actions.declare_output(
        "__preprocessed__",
        "{}.{}".format(filename_base, "i"),
        has_content_based_path = content_based,
    )

    # External debug info
    external_debug_info = None
    extension_supports_external_debug_info = src_compile_cmd.src.extension not in (".hip")
    use_external_debug_info = (
        separate_debug_info and toolchain.split_debug_mode == SplitDebugMode("split") and compiler_type == "clang" and extension_supports_external_debug_info
    )
    if use_external_debug_info:
        external_debug_info = actions.declare_output(
            folder_name,
            "{}.{}".format(filename_base, "dwo"),
            has_content_based_path = content_based,
        )

    # .S extension is native assembly code (machine level, processor specific)
    # and clang will happily compile them to .o files, but the object are always
    # native even if we ask for bitcode.  If we don't mark the output format,
    # other tools would try and parse the .o file as LLVM-IR and fail.
    if src_compile_cmd.src.extension in [".S", ".s"]:
        object_format = CxxObjectFormat("native")
    else:
        object_format = default_object_format

    assembly = _declare_assembly(
        actions = actions,
        compiler_type = compiler_type,
        object_format = object_format,
        filename_base = filename_base,
        content_based = content_based,
    )

    # If we're building with split debugging, where the debug info is in the
    # original object, then add the object as external debug info
    # FIXME: ThinLTO generates debug info in a separate dwo dir, but we still
    # need to track object files if the object file is not compiled to bitcode.
    # We should track whether ThinLTO is used on a per-object basis rather than
    # globally on a toolchain level.
    object_has_external_debug_info = toolchain.split_debug_mode == SplitDebugMode("single")

    clang_trace = _declare_clang_trace_output(toolchain, compiler_type, actions, filename_base, content_based)

    # Only declare CUDA distributed compile outputs for CUDA source files
    cuda_dist_output = None
    if src_compile_cmd.src.extension == ".cu" and cuda_compile_style == CudaCompileStyle("dist"):
        cuda_dist_output = shared_cuda_dist_output or declare_cuda_dist_compile_output(
            actions,
            CudaCompileInfo(
                filename = filename_base,
                identifier = identifier,
                output_prefix = folder_name,
                uses_content_based_paths = content_based,
            ),
        )

    # Declare PCH object output for Windows compiler
    pch_object = None
    if compiler_type == "windows" and compile_pch:
        pch_object = actions.declare_output(
            folder_name,
            "{}.pch.o".format(filename_base),
            has_content_based_path = content_based,
        )

    # JSON error output
    json_error = None
    serialized_diags_to_json = toolchain.binary_utilities_info.custom_tools.get("serialized-diags-to-json", None)
    if serialized_diags_to_json and src_compile_cmd.error_handler and compiler_type == "clang" and src_compile_cmd.src.extension != ".cu":
        json_error = actions.declare_output(
            "__diagnostics__/{}.json".format(filename_base),
            has_content_based_path = content_based,
        )

    hip_device_debug = src_compile_cmd.src.extension == ".hip" and toolchain.hip_device_debug_extract

    declared_artifacts = CxxCompileOutput(
        object = object,
        object_format = object_format,
        object_has_external_debug_info = object_has_external_debug_info,
        external_debug_info = external_debug_info,
        clang_remarks = clang_remarks,
        clang_llvm_statistics = clang_llvm_statistics,
        clang_trace = clang_trace,
        gcno_file = gcno_file,
        index_store = index_store,
        assembly = assembly,
        diagnostics = diagnostics,
        clang_tidy_diagnostics = clang_tidy_diagnostics,
        preproc = preproc,
        dist_cuda = cuda_dist_output,
        pch_object = pch_object,
        json_error = json_error,
        hip_device_debug = hip_device_debug,
    )

    info = CxxCompileInfo(
        src = src_compile_cmd.src,
        args = src_compile_cmd.args,
        short_path = short_path,
        filename_base = filename_base,
        index_store_base = index_store_base,
        identifier = identifier,
        folder_name = folder_name,
        weight = src_compile_cmd.weight,
    )

    return CxxCompileInput(
        declared_artifacts = declared_artifacts,
        info = info,
    )

def _compile_single_cxx(
    actions: AnalysisActions,
    label: Label,
    toolchain: CxxToolchainInfo,
    flavors: set[CxxCompileFlavor],
    compile_pch: CxxPrecompiledHeader | None,
    precompiled_header: Dependency | None,
    cuda_compile_style: CudaCompileStyle | None,
    use_header_units: UseHeaderUnitsMode,
    # CxxCompileInfo fields
    info: CxxCompileInfo,
    shared_info: CxxSharedCompileInfo,
    # Output artifacts
    object: OutputArtifact,
    external_debug_info: OutputArtifact | None,
    clang_remarks: OutputArtifact | None,
    clang_llvm_statistics: OutputArtifact | None,
    clang_trace: OutputArtifact | None,
    gcno_file: OutputArtifact | None,
    assembly: OutputArtifact | None,
    diagnostics: OutputArtifact | None,
    clang_tidy_diagnostics: OutputArtifact | None,
    preproc: OutputArtifact,
    index_store: OutputArtifact | None,
    dist_cuda: CudaDistributedCompileOutput | None,
    prepare_cuda_dist: bool,
    cuda_shared_plan_identifier: str | None,
    shared_compile_args: CxxSharedCompileCommandArgs,
    shared_auxiliary_args: CxxSharedCompileCommandArgs,
    shared_dryrun_args: CxxSharedCompileCommandArgs | None,
    pch_object: OutputArtifact | None,
    json_error: OutputArtifact | None,
) -> CudaDistributedCompileSpec | None:
    """
    EXECUTION PHASE: Creates compilation actions for a single source file.

    This function runs INSIDE the dynamic action callback (`_cxx_dynamic_compile`),
    meaning it only executes when Buck2 determines the compilation is actually needed.

    ## Relationship to Declaration Phase
    The output artifacts passed here were declared earlier by `_prepare_cxx_compilation`
    during the analysis phase.
    """

    # Extract info fields
    src = info.src
    src_args = info.args
    filename_base = info.filename_base
    index_store_base = info.index_store_base
    identifier = info.identifier
    folder_name = info.folder_name
    short_path = info.short_path
    cxx_compile_cmd = shared_info.cxx_compile_cmd

    # Get compiler type
    compiler_type = cxx_compile_cmd.compiler_type
    content_based = cxx_compile_cmd.allow_content_based_paths
    uses_header_unit_pcms = use_header_units == UseHeaderUnitsMode("pcm") and cxx_compile_cmd.header_units_argsfile != None

    cuda_dist_spec = None
    if src.extension == ".cu":
        output_args = None
    elif compile_pch:
        if compiler_type == "windows":
            output_args = [
                cmd_args(object, format = "/Fp{}"),
                cmd_args(pch_object, format = "/Fo{}"),
                cmd_args(compile_pch.path, format = "/Yc{}"),
            ]
        else:
            output_args = [
                get_output_flags(compiler_type, object),
                "-Xclang",
                "-emit-pch",
                "-Xclang",
                "-fno-pch-timestamp",
                "-fpch-instantiate-templates",
            ]
    else:
        output_args = get_output_flags(compiler_type, object)

    base_compile_cmd_override = None
    if src.extension == ".cu" and cuda_compile_style == CudaCompileStyle("dist"):
        compiler_for_dryrun = getattr(toolchain.cuda_compiler_info, "compiler_for_dryrun", None)
        if compiler_for_dryrun != None:
            base_compile_cmd_override = _get_compile_base(
                toolchain,
                toolchain.cuda_compiler_info,
                use_wrapper = False,
                compiler_override = compiler_for_dryrun,
            )

    cmd = _get_base_compile_cmd(
        src = src,
        src_args = src_args,
        cxx_compile_cmd = cxx_compile_cmd,
        output_args = output_args,
        base_compile_cmd_override = base_compile_cmd_override,
        shared_args = shared_compile_args,
    )
    cuda_prepare_cmd = None
    if prepare_cuda_dist:
        expect(shared_dryrun_args != None, "distributed CUDA prepare requires dry-run compile args")
        cuda_prepare_cmd = _get_base_compile_cmd(
            src = src,
            src_args = src_args,
            cxx_compile_cmd = cxx_compile_cmd,
            base_compile_cmd_override = base_compile_cmd_override,
            source_override = _CUDA_DRYRUN_SOURCE_PLACEHOLDER,
            shared_args = shared_dryrun_args,
        )

    if index_store:
        compile_index_store_cmd = _get_base_compile_cmd(
            src = src,
            src_args = src_args,
            cxx_compile_cmd = cxx_compile_cmd,
            shared_args = shared_auxiliary_args,
        )
        shared_info.index_store_factory.compile(
            actions,
            label,
            index_store,
            index_store_base,
            toolchain,
            compile_index_store_cmd,
        )

    if precompiled_header and precompiled_header[CPrecompiledHeaderInfo] and precompiled_header[CPrecompiledHeaderInfo].compiled:
        pch_info = precompiled_header[DefaultInfo].sub_targets["pch"]
        pch_subtargets = pch_info.get(DefaultInfo).sub_targets

        pch_flavor = "default"
        for flavor in flavors:
            pch_flavor = flavor.value if flavor.value else pch_flavor

        target = pch_subtargets[pch_flavor].get(CPrecompiledHeaderInfo)
        cmd.add(_get_use_pch_args(src, cxx_compile_cmd, target, precompiled_header[CPrecompiledHeaderInfo]))

    action_dep_files = {}

    headers_dep_files = cxx_compile_cmd.headers_dep_files

    # CUDA compilation attaches dep files itself (see cuda_mono_compile and
    # cuda_distributed_compile), because the flags have to land on the real
    # compile sub-command rather than on the nvcc driver command we dryrun here.
    if src.extension == ".cu":
        headers_dep_files = None

    if headers_dep_files:
        cmd = add_headers_dep_files(
            actions,
            cmd,
            headers_dep_files,
            src,
            filename_base,
            action_dep_files,
        )
        if uses_header_unit_pcms:
            cmd.add("-fmodule-file-deps")

    if clang_remarks:
        cmd.add(["-fsave-optimization-record", "-fdiagnostics-show-hotness", "-foptimization-record-passes=" + toolchain.clang_remarks])
        cmd.add(cmd_args(hidden = clang_remarks))

    if clang_llvm_statistics:
        # Use stderr_to_file to capture clang statistics output
        cmd = cmd_args(
            toolchain.internal_tools.stderr_to_file,
            cmd_args(clang_llvm_statistics, format = "--out={}"),
            cmd,
            ["-mllvm", "-stats"],
        )

    if clang_trace:
        cmd.add(["-ftime-trace"])
        cmd.add(cmd_args(hidden = clang_trace))

    if gcno_file:
        cmd.add(["--coverage"])
        cmd.add(cmd_args(hidden = gcno_file))

    if external_debug_info:
        cmd.add(cmd_args(external_debug_info, format = "--fbcc-create-external-debug-info={}"))

    outputs_for_error_handler = []
    if json_error:
        # We need to wrap the entire compile to provide serialized diagnostics
        # output and on error convert it to JSON.
        serialized_diags_to_json = toolchain.binary_utilities_info.custom_tools.get("serialized-diags-to-json", None)
        outputs_for_error_handler.append(json_error)
        cmd = cmd_args(
            toolchain.internal_tools.serialized_diagnostics_to_json_wrapper,
            serialized_diags_to_json,
            json_error,
            cmd,
        )

    if src.extension == ".cu":
        expect(cuda_compile_style != None, "CUDA compile style should be configured for targets with .cu sources")
        cuda_compile_info = CudaCompileInfo(
            filename = filename_base,
            identifier = identifier,
            output_prefix = folder_name,
            uses_content_based_paths = content_based,
        )

        cuda_dist_spec = cuda_compile(
            actions,
            cmd,
            object,
            src,
            cxx_compile_cmd,
            cuda_compile_info,
            action_dep_files,
            allow_dep_file_cache_upload = toolchain.allow_dep_file_cache_upload,
            error_handler = shared_info.error_handler,
            cuda_compile_style = cuda_compile_style,
            cuda_dist_output = dist_cuda,
            cuda_prepare_cmd = cuda_prepare_cmd,
            shared_plan_identifier = cuda_shared_plan_identifier,
        )
    else:
        is_producing_compiled_pch = bool(compile_pch)
        is_consuming_compiled_pch = bool(precompiled_header and precompiled_header[CPrecompiledHeaderInfo].compiled)
        actions.run(
            cmd,
            category = cxx_compile_cmd.category,
            identifier = identifier,
            dep_files = action_dep_files,
            allow_cache_upload = cxx_compile_cmd.allow_cache_upload,
            allow_dep_file_cache_upload = toolchain.allow_dep_file_cache_upload,
            error_handler = shared_info.error_handler,
            outputs_for_error_handler = outputs_for_error_handler,
            local_only = is_producing_compiled_pch or is_consuming_compiled_pch,
            weight = info.weight,
            expect_eligible_for_dedupe = shared_info.expect_eligible_for_dedupe,
        )

    # Generate asm for compiler which accept `-S` (TODO: support others)
    if assembly:
        assembly_cmd = _get_base_compile_cmd(
            src = src,
            src_args = src_args,
            cxx_compile_cmd = cxx_compile_cmd,
            output_args = ["-S"] + get_output_flags(compiler_type, assembly),
            shared_args = shared_auxiliary_args,
        )
        actions.run(
            assembly_cmd,
            category = cxx_compile_cmd.category,
            identifier = identifier + " (assembly)",
            allow_cache_upload = cxx_compile_cmd.allow_cache_upload,
            allow_dep_file_cache_upload = False,
            error_handler = shared_info.error_handler,
        )

    if diagnostics:
        syntax_only_cmd = _get_base_compile_cmd(
            src = src,
            src_args = src_args,
            cxx_compile_cmd = cxx_compile_cmd,
            output_args = ["-fsyntax-only"],
            shared_args = shared_auxiliary_args,
        )
        diagnostics_dep_files = {}
        if headers_dep_files:
            syntax_only_cmd = add_headers_dep_files(
                actions = actions,
                cmd = syntax_only_cmd,
                headers_dep_files = headers_dep_files,
                src = src,
                filename_base = "{}.check".format(filename_base),
                action_dep_files = diagnostics_dep_files,
            )
            if uses_header_unit_pcms:
                syntax_only_cmd.add("-fmodule-file-deps")
        actions.run(
            [
                toolchain.internal_tools.stderr_to_file,
                cmd_args(diagnostics, format = "--out={}"),
                syntax_only_cmd,
            ],
            category = "check",
            identifier = short_path,
            dep_files = diagnostics_dep_files,
            allow_cache_upload = cxx_compile_cmd.allow_cache_upload,
            allow_dep_file_cache_upload = toolchain.allow_dep_file_cache_upload,
            error_handler = shared_info.error_handler,
        )

    if clang_tidy_diagnostics:
        clang_tidy_tool = toolchain.binary_utilities_info.custom_tools["clang-tidy"]
        actions.run(
            [
                toolchain.internal_tools.clang_tidy_wrapper,
                cmd_args(clang_tidy_diagnostics, format = "--output={}"),
                cmd_args(clang_tidy_tool, format = "--clang-tidy={}"),
                cmd_args(src, format = "--source={}"),
                "--",
                cxx_compile_cmd.base_compile_cmd,
                cxx_compile_cmd.argsfile.cmd_form,
                src_args,
            ],
            category = "clang_tidy",
            identifier = short_path,
            allow_cache_upload = cxx_compile_cmd.allow_cache_upload,
            allow_dep_file_cache_upload = False,
            error_handler = shared_info.error_handler,
        )

    # Generate pre-processed sources
    preproc_cmd = _get_base_compile_cmd(
        src = src,
        src_args = src_args,
        cxx_compile_cmd = cxx_compile_cmd,
        output_args = [COMMON_PREPROCESSOR_OUTPUT_ARGS, get_output_flags(compiler_type, preproc)],
        shared_args = shared_auxiliary_args,
    )
    actions.run(
        preproc_cmd,
        category = cxx_compile_cmd.category,
        identifier = identifier + " (preprocessor)",
        allow_cache_upload = cxx_compile_cmd.allow_cache_upload,
        allow_dep_file_cache_upload = False,
        error_handler = shared_info.error_handler,
    )

    return cuda_dist_spec

def _get_base_compile_cmd(
    src: Artifact,
    src_args: list[typing.Any],
    cxx_compile_cmd: CxxCompileCommand,
    shared_args: CxxSharedCompileCommandArgs,
    output_args: list | None = None,
    base_compile_cmd_override = None,
    source_override: str | None = None,
) -> cmd_args:
    """
    Construct a compile command for a single CXX source from its
    `cxx_compile_cmd`, the extension-shared `shared_args`, and other
    compilation options.
    """
    cmd = cmd_args(base_compile_cmd_override if base_compile_cmd_override != None else cxx_compile_cmd.base_compile_cmd)
    if output_args:
        cmd.add(output_args)

    cmd.add(shared_args.before_src)

    if source_override == None:
        cmd.add(src_args)
    else:
        expect(
            src_args and src_args[-1] == src,
            "source override requires the source artifact to be the final compile argument",
        )
        cmd.add(src_args[:-1])
        cmd.add(source_override)

    cmd.add(shared_args.after_src)

    return cmd

def _get_shared_compile_command_args(
    bitcode_args: cmd_args | list,
    cxx_compile_cmd: CxxCompileCommand,
    flavors: set[CxxCompileFlavor],
    flavor_flags: dict[str, list[str]],
    use_header_units: UseHeaderUnitsMode = UseHeaderUnitsMode("none"),
    cuda_dryrun: bool = False,
) -> CxxSharedCompileCommandArgs:
    before_src = cmd_args()

    if use_header_units == UseHeaderUnitsMode("pcm") and cxx_compile_cmd.header_units_argsfile:
        before_src.add(cxx_compile_cmd.header_units_argsfile.cmd_form)
    elif use_header_units == UseHeaderUnitsMode("stub") and cxx_compile_cmd.header_unit_stubs_argsfile:
        before_src.add(cmd_args(hidden = cxx_compile_cmd.header_unit_stubs_argsfile.file))

    if cuda_dryrun:
        dryrun_form = cxx_compile_cmd.argsfile.cmd_form_cuda_dryrun
        expect(dryrun_form != None, "compile argsfile has no CUDA dry-run form")
        before_src.add(dryrun_form)
    else:
        before_src.add(cxx_compile_cmd.argsfile.cmd_form)

    for flavor in flavors:
        flags = flavor_flags.get(flavor.value)
        if flags:
            before_src.add(flags)

    return CxxSharedCompileCommandArgs(
        before_src = before_src,
        after_src = cmd_args(bitcode_args),
    )

def toolchain_supports_flavor(toolchain: CxxToolchainInfo, flavor: CxxCompileFlavor) -> bool:
    return flavor.value in toolchain.supported_compile_flavors and toolchain.compiler_flavor_flags.get(flavor.value) != None

# Fallback when the toolchain configures no flavor flags at all, for clang and
# gcc. Ideally this would be a default value in the toolchain definition itself,
# but it is compiler-dependent so we can't have it there. Module-level so every
# per-source compile record shares one frozen value instead of allocating a
# fresh dict+list per TU.
_DEFAULT_PIC_FLAVOR_FLAGS = {CxxCompileFlavor("pic").value: ["-fPIC"]}

def build_flavor_flags(flavor_flags: dict[str, typing.Any], compiler_type: str) -> dict[str, typing.Any]:
    if not flavor_flags and compiler_type in ["clang", "gcc"]:
        return _DEFAULT_PIC_FLAVOR_FLAGS

    return flavor_flags

def _cxx_dynamic_compile(
    actions: AnalysisActions,
    label: Label,
    toolchain: CxxToolchainInfo,
    bitcode_args: list[str],
    flavors: list[CxxCompileFlavor],
    use_header_units: UseHeaderUnitsMode,
    precompiled_header: Dependency | None,
    compile_pch: CxxPrecompiledHeader | None,
    cuda_compile_style: CudaCompileStyle | None,
    infos: list[CxxCompileInfo],
    shared_infos: dict[str, CxxSharedCompileInfo],
    object: list[OutputArtifact],
    external_debug_info: dict[int, OutputArtifact],
    clang_remarks: dict[int, OutputArtifact],
    clang_llvm_statistics: dict[int, OutputArtifact],
    clang_trace: dict[int, OutputArtifact],
    gcno_file: dict[int, OutputArtifact],
    assembly: list[OutputArtifact | None],
    diagnostics: list[OutputArtifact | None],
    clang_tidy_diagnostics: dict[int, OutputArtifact],
    preproc: list[OutputArtifact],
    index_store: dict[int, OutputArtifact],
    dist_cuda: dict[int, (OutputArtifact, OutputArtifact, OutputArtifact)],
    shared_dist_cuda: (OutputArtifact, OutputArtifact, OutputArtifact) | None,
    pch_object: dict[int, OutputArtifact],
    json_error: dict[int, OutputArtifact],
) -> list[Provider]:
    """
    DYNAMIC ACTION CALLBACK: The bridge between declaration and execution phases.

    This function is the implementation for Buck2's `dynamic_output_new` API. It runs
    LAZILY—only when Buck2 determines that one or more of the declared outputs are
    actually needed by the build. This is the key to memory optimization: action
    graphs are not materialized until necessary.

    ## How This Fits
    ```
    compile_cxx()                          # ANALYSIS PHASE
        │
        ├─► for each source file:
        │       _prepare_cxx_compilation() # Declares outputs (Artifact)
        │       collect declared artifacts
        │
        └─► actions.dynamic_output_new()   # Registers this callback
                    │
                    ▼
            _cxx_dynamic_compile()         # EXECUTION PHASE (called lazily)
                    │
                    └─► for each source:
                            _compile_single_cxx()  # Creates actual actions
    ```

    ## Why Batched Dynamic Actions?
    All source files are compiled in a single `dynamic_output_new` call rather than
    one per source file.
    """
    flavors_set = set(flavors)
    shared_cuda_specs = []
    shared_cuda_dist_output = None
    shared_cuda_plan_src_idx = None
    if shared_dist_cuda != None:
        shared_cuda_dist_output = CudaDistributedCompileOutput(
            nvcc_dag = shared_dist_cuda[0].as_input(),
            nvcc_env = shared_dist_cuda[1].as_input(),
            hostcc_argsfile = shared_dist_cuda[2].as_input(),
        )
    cuda_shared_plan_identifier = _cuda_plan_identifier(flavors_set) if shared_cuda_dist_output != None else None
    shared_compile_args = {}
    shared_auxiliary_args = {}
    shared_dryrun_args = {}
    for extension, shared_info in shared_infos.items():
        flavor_flags = build_flavor_flags(toolchain.compiler_flavor_flags, shared_info.cxx_compile_cmd.compiler_type)
        shared_compile_args[extension] = _get_shared_compile_command_args(
            bitcode_args = bitcode_args,
            cxx_compile_cmd = shared_info.cxx_compile_cmd,
            flavors = flavors_set,
            flavor_flags = flavor_flags,
            use_header_units = use_header_units,
        )
        shared_auxiliary_args[extension] = _get_shared_compile_command_args(
            bitcode_args = bitcode_args,
            cxx_compile_cmd = shared_info.cxx_compile_cmd,
            flavors = flavors_set,
            flavor_flags = toolchain.compiler_flavor_flags,
        )
        if extension == ".cu" and cuda_compile_style == CudaCompileStyle("dist"):
            shared_dryrun_args[extension] = _get_shared_compile_command_args(
                bitcode_args = bitcode_args,
                cxx_compile_cmd = shared_info.cxx_compile_cmd,
                flavors = flavors_set,
                flavor_flags = flavor_flags,
                use_header_units = use_header_units,
                cuda_dryrun = True,
            )
    for i in range(len(infos)):
        extension = infos[i].src.extension
        is_cuda = extension == ".cu"

        # Exactly one prepare action must bind the shared plan outputs. Any
        # shared CUDA source is a valid representative, since sharing requires
        # identical plan-affecting args across sources; use the first one.
        if is_cuda and shared_cuda_dist_output != None and shared_cuda_plan_src_idx == None:
            shared_cuda_plan_src_idx = i
        source_dist_cuda = shared_cuda_dist_output if is_cuda else None
        source_dist_cuda_outputs = dist_cuda.get(i)
        if source_dist_cuda == None and source_dist_cuda_outputs != None:
            source_dist_cuda = CudaDistributedCompileOutput(
                nvcc_dag = source_dist_cuda_outputs[0].as_input(),
                nvcc_env = source_dist_cuda_outputs[1].as_input(),
                hostcc_argsfile = source_dist_cuda_outputs[2].as_input(),
            )
        prepare_cuda_dist = source_dist_cuda != None and (shared_cuda_dist_output == None or i == shared_cuda_plan_src_idx)
        cuda_dist_spec = _compile_single_cxx(
            actions = actions,
            label = label,
            toolchain = toolchain,
            flavors = flavors_set,
            compile_pch = compile_pch,
            precompiled_header = precompiled_header,
            cuda_compile_style = cuda_compile_style,
            use_header_units = use_header_units,
            info = infos[i],
            shared_info = shared_infos[extension],
            object = object[i],
            external_debug_info = external_debug_info.get(i),
            clang_remarks = clang_remarks.get(i),
            clang_llvm_statistics = clang_llvm_statistics.get(i),
            clang_trace = clang_trace.get(i),
            gcno_file = gcno_file.get(i),
            assembly = assembly[i],
            diagnostics = diagnostics[i],
            clang_tidy_diagnostics = clang_tidy_diagnostics.get(i),
            preproc = preproc[i],
            index_store = index_store.get(i),
            dist_cuda = source_dist_cuda,
            prepare_cuda_dist = prepare_cuda_dist,
            cuda_shared_plan_identifier = cuda_shared_plan_identifier,
            shared_compile_args = shared_compile_args[extension],
            shared_auxiliary_args = shared_auxiliary_args[extension],
            shared_dryrun_args = shared_dryrun_args.get(extension),
            pch_object = pch_object.get(i),
            json_error = json_error.get(i),
        )
        if cuda_dist_spec != None:
            if shared_cuda_dist_output != None:
                shared_cuda_specs.append(cuda_dist_spec)
            else:
                cxx_compile_cmd = shared_infos[extension].cxx_compile_cmd
                create_cuda_distributed_compiles(
                    actions,
                    toolchain,
                    source_dist_cuda,
                    [cuda_dist_spec],
                    cxx_compile_cmd.allow_cache_upload,
                    # The argsfile closure carries the file-prefix specs file
                    # that replayed host compiles reference by path.
                    cmd_args(cxx_compile_cmd.base_compile_cmd, hidden = cxx_compile_cmd.argsfile.cmd_form),
                    cxx_compile_cmd.headers_dep_files,
                )

    if shared_cuda_specs:
        cxx_compile_cmd = shared_infos[".cu"].cxx_compile_cmd
        expect(
            shared_cuda_plan_src_idx != None,
            "shared CUDA plan outputs were declared but no .cu source is present to bind them",
        )
        create_cuda_distributed_compiles(
            actions,
            toolchain,
            shared_cuda_dist_output,
            shared_cuda_specs,
            cxx_compile_cmd.allow_cache_upload,
            cmd_args(cxx_compile_cmd.base_compile_cmd, hidden = cxx_compile_cmd.argsfile.cmd_form),
            cxx_compile_cmd.headers_dep_files,
        )

    return [EMPTY_DEFAULT_INFO]

# https://buck2.build/docs/api/build/AnalysisActions/#analysisactionsdynamic_output_new
# Dynamic actions factory for batch CXX compilation
_dynamic_compile_rule = dynamic_actions(
    impl = _cxx_dynamic_compile,
    attrs = {
        "assembly": dynattrs.list(dynattrs.option(dynattrs.output())),
        "bitcode_args": dynattrs.value(list[str]),
        "clang_llvm_statistics": dynattrs.dict(int, dynattrs.output()),
        "clang_remarks": dynattrs.dict(int, dynattrs.output()),
        "clang_tidy_diagnostics": dynattrs.dict(int, dynattrs.output()),
        "clang_trace": dynattrs.dict(int, dynattrs.output()),
        "compile_pch": dynattrs.option(dynattrs.value(CxxPrecompiledHeader)),
        "cuda_compile_style": dynattrs.option(dynattrs.value(CudaCompileStyle)),
        "diagnostics": dynattrs.list(dynattrs.option(dynattrs.output())),
        "dist_cuda": dynattrs.dict(int, dynattrs.tuple(dynattrs.output(), dynattrs.output(), dynattrs.output())),
        "external_debug_info": dynattrs.dict(int, dynattrs.output()),
        "flavors": dynattrs.value(list[CxxCompileFlavor]),
        "gcno_file": dynattrs.dict(int, dynattrs.output()),
        "index_store": dynattrs.dict(int, dynattrs.output()),
        "infos": dynattrs.value(list[CxxCompileInfo]),
        "json_error": dynattrs.dict(int, dynattrs.output()),
        "label": dynattrs.value(Label),
        "object": dynattrs.list(dynattrs.output()),
        "pch_object": dynattrs.dict(int, dynattrs.output()),
        "precompiled_header": dynattrs.option(dynattrs.value(Dependency)),
        "preproc": dynattrs.list(dynattrs.output()),
        "shared_dist_cuda": dynattrs.option(dynattrs.tuple(dynattrs.output(), dynattrs.output(), dynattrs.output())),
        "shared_infos": dynattrs.dict(str, dynattrs.value(CxxSharedCompileInfo)),
        "toolchain": dynattrs.value(CxxToolchainInfo),
        "use_header_units": dynattrs.value(UseHeaderUnitsMode),
    },
)

def _cuda_plan_flavor(flavors: set[CxxCompileFlavor]) -> str:
    return ".".join(sorted([flavor.value for flavor in flavors])) or "default"

def _cuda_plan_identifier(flavors: set[CxxCompileFlavor]) -> str:
    return "cuda_compile_plan ({})".format(_cuda_plan_flavor(flavors))

def maybe_get_shared_cuda_dist_output(
    actions: AnalysisActions,
    src_compile_cmds: list[CxxSrcCompileCommand],
    cuda_compile_style: CudaCompileStyle | None,
    flavors: set[CxxCompileFlavor],
) -> CudaDistributedCompileOutput | None:
    cuda_src_compile_cmds = [cmd for cmd in src_compile_cmds if cmd.src.extension == ".cu"]
    if cuda_compile_style != CudaCompileStyle("dist") or not cuda_src_compile_cmds:
        return None

    for cmd in cuda_src_compile_cmds:
        # Same-extension sources already share one `cxx_compile_cmd`, so per-source
        # args, duplicate-basename disambiguation (`index`), and path style are the
        # only sharing hazards.
        # create_compile_cmds appends source flags before "-c" and the source artifact.
        # Only commands with no source-specific flags can share an NVCC plan.
        if len(cmd.args) != 2 or cmd.args[0] != "-c" or cmd.args[1] != cmd.src or cmd.index != None:
            return None
        if cmd.uses_content_based_paths != cuda_src_compile_cmds[0].uses_content_based_paths:
            return None

    # Plan outputs are named per flavor set: callers invoke compile_cxx at most
    # once per flavor set within a target, and a collision fails analysis loudly.
    cuda_plan_flavor = _cuda_plan_flavor(flavors)
    return declare_cuda_dist_compile_output(
        actions,
        CudaCompileInfo(
            filename = "cuda_compile_plan.{}".format(cuda_plan_flavor),
            identifier = _cuda_plan_identifier(flavors),
            output_prefix = _OBJECTS_FOLDER,
            uses_content_based_paths = cuda_src_compile_cmds[0].uses_content_based_paths,
        ),
    )

def compile_cxx(
    actions: AnalysisActions,
    target_label: Label,
    toolchain: CxxToolchainInfo,
    src_compile_cmds: list[CxxSrcCompileCommand],
    flavors: set[CxxCompileFlavor],
    provide_syntax_only: bool,
    separate_debug_info: bool,
    use_header_units: UseHeaderUnitsMode = UseHeaderUnitsMode("none"),
    precompiled_header: Dependency | None = None,
    cuda_compile_style: CudaCompileStyle | None = None,
    compile_pch: CxxPrecompiledHeader | None = None,
) -> list[CxxCompileOutput]:
    """
    MAIN ENTRY POINT: Compiles a list of C/C++ source files.

    This function implements a two-phases that separates output
    declaration (analysis phase) from action creation (execution phase).
    """
    linker_info = toolchain.linker_info

    # Resolve the output format, which is a tristate of native (default being mach-o/elf/pe)
    # bitcode (being LLVM-IR, which is also produced if any link time optimization flags are
    # enabled) or the third hybrid state where the bitcode is embedded into a section of the
    # native code, allowing the file to be used as either (but at twice the size)
    default_object_format = toolchain.object_format or CxxObjectFormat("native")
    bitcode_args = []
    if linker_info.lto_mode == LtoMode("none"):
        if toolchain.object_format == CxxObjectFormat("bitcode"):
            bitcode_args.append("-emit-llvm")
            default_object_format = CxxObjectFormat("bitcode")
        elif toolchain.object_format == CxxObjectFormat("embedded-bitcode"):
            bitcode_args.append("-fembed-bitcode")
            default_object_format = CxxObjectFormat("embedded-bitcode")
    else:
        # LTO always produces bitcode object in any mode (thin, full, etc)
        default_object_format = CxxObjectFormat("bitcode")

    objects = []

    # Dynamic action inputs
    infos = []
    object_outputs = []
    external_debug_info_outputs = {}
    clang_remarks_outputs = {}
    clang_llvm_statistics_outputs = {}
    clang_trace_outputs = {}
    gcno_file_outputs = {}
    assembly_outputs = []
    diagnostics_outputs = []
    clang_tidy_diagnostics_outputs = {}
    preproc_outputs = []
    index_store_outputs = {}
    dist_cuda_outputs = {}
    pch_object_outputs = {}
    json_error_outputs = {}
    shared_infos = {}

    for src_compile_cmd in src_compile_cmds:
        extension = src_compile_cmd.src.extension
        shared_info = shared_infos.get(extension)
        if shared_info == None:
            shared_infos[extension] = CxxSharedCompileInfo(
                cxx_compile_cmd = src_compile_cmd.cxx_compile_cmd,
                index_store_factory = src_compile_cmd.index_store_factory,
                error_handler = src_compile_cmd.error_handler,
                expect_eligible_for_dedupe = src_compile_cmd.expect_eligible_for_dedupe,
            )
        else:
            expect(
                src_compile_cmd.cxx_compile_cmd == shared_info.cxx_compile_cmd
                and src_compile_cmd.index_store_factory == shared_info.index_store_factory
                and src_compile_cmd.error_handler == shared_info.error_handler
                and src_compile_cmd.expect_eligible_for_dedupe == shared_info.expect_eligible_for_dedupe,
                "sources with the same extension must share compile command inputs",
            )

    shared_cuda_dist_output = maybe_get_shared_cuda_dist_output(
        actions,
        src_compile_cmds,
        cuda_compile_style,
        flavors,
    )

    for source_index, src_compile_cmd in enumerate(src_compile_cmds):
        cxx_compile_input = _prepare_cxx_compilation(
            actions = actions,
            toolchain = toolchain,
            default_object_format = default_object_format,
            src_compile_cmd = src_compile_cmd,
            flavors = flavors,
            provide_syntax_only = provide_syntax_only,
            use_header_units = use_header_units,
            separate_debug_info = separate_debug_info,
            cuda_compile_style = cuda_compile_style,
            shared_cuda_dist_output = shared_cuda_dist_output,
            compile_pch = compile_pch,
        )

        # Collect info (metadata without artifacts)
        infos.append(cxx_compile_input.info)

        # Collect outputs - call .as_output() on each artifact
        declared = cxx_compile_input.declared_artifacts
        object_outputs.append(declared.object.as_output())
        if declared.external_debug_info:
            external_debug_info_outputs[source_index] = declared.external_debug_info.as_output()
        if declared.clang_remarks:
            clang_remarks_outputs[source_index] = declared.clang_remarks.as_output()
        if declared.clang_llvm_statistics:
            clang_llvm_statistics_outputs[source_index] = declared.clang_llvm_statistics.as_output()
        if declared.clang_trace:
            clang_trace_outputs[source_index] = declared.clang_trace.as_output()
        if declared.gcno_file:
            gcno_file_outputs[source_index] = declared.gcno_file.as_output()
        assembly_outputs.append(map_val(as_output, declared.assembly))
        diagnostics_outputs.append(map_val(as_output, declared.diagnostics))
        if declared.clang_tidy_diagnostics:
            clang_tidy_diagnostics_outputs[source_index] = declared.clang_tidy_diagnostics.as_output()
        preproc_outputs.append(declared.preproc.as_output())
        if declared.index_store:
            index_store_outputs[source_index] = declared.index_store.as_output()
        if shared_cuda_dist_output == None and declared.dist_cuda:
            dist_cuda_outputs[source_index] = (
                declared.dist_cuda.nvcc_dag.as_output(),
                declared.dist_cuda.nvcc_env.as_output(),
                declared.dist_cuda.hostcc_argsfile.as_output(),
            )
        if declared.pch_object:
            pch_object_outputs[source_index] = declared.pch_object.as_output()
        if declared.json_error:
            json_error_outputs[source_index] = declared.json_error.as_output()

        objects.append(declared)

    actions.dynamic_output_new(
        _dynamic_compile_rule(
            assembly = assembly_outputs,
            bitcode_args = bitcode_args,
            clang_llvm_statistics = clang_llvm_statistics_outputs,
            clang_remarks = clang_remarks_outputs,
            clang_tidy_diagnostics = clang_tidy_diagnostics_outputs,
            clang_trace = clang_trace_outputs,
            compile_pch = compile_pch,
            cuda_compile_style = cuda_compile_style,
            diagnostics = diagnostics_outputs,
            dist_cuda = dist_cuda_outputs,
            external_debug_info = external_debug_info_outputs,
            flavors = list(flavors),
            gcno_file = gcno_file_outputs,
            index_store = index_store_outputs,
            infos = infos,
            json_error = json_error_outputs,
            label = target_label,
            object = object_outputs,
            pch_object = pch_object_outputs,
            precompiled_header = precompiled_header,
            preproc = preproc_outputs,
            shared_dist_cuda = map_val(lambda d: (d.nvcc_dag.as_output(), d.nvcc_env.as_output(), d.hostcc_argsfile.as_output()), shared_cuda_dist_output),
            shared_infos = shared_infos,
            toolchain = toolchain,
            use_header_units = use_header_units,
        )
    )

    # Strip per-arch device DWARF from each .hip TU's `.o` into
    # sidecars. Replaces `.object` with the stripped artifact; downstream
    # linker concatenates the stripped bundles into the final
    # `.hip_fatbin`.
    if hip_debug_extract_available(toolchain):
        processed = []
        for i, out in enumerate(objects):
            if out.hip_device_debug:
                hip_outs = hip_debug_extract_compile_object(
                    actions = actions,
                    toolchain = toolchain,
                    obj = out.object,
                    identifier = infos[i].identifier,
                )
                processed.append(
                    CxxCompileOutput(
                        object = hip_outs.patched_output,
                        object_format = out.object_format,
                        object_has_external_debug_info = out.object_has_external_debug_info,
                        external_debug_info = out.external_debug_info,
                        clang_remarks = out.clang_remarks,
                        clang_llvm_statistics = out.clang_llvm_statistics,
                        clang_trace = out.clang_trace,
                        gcno_file = out.gcno_file,
                        index_store = out.index_store,
                        assembly = out.assembly,
                        diagnostics = out.diagnostics,
                        clang_tidy_diagnostics = out.clang_tidy_diagnostics,
                        preproc = out.preproc,
                        dist_cuda = out.dist_cuda,
                        pch_object = out.pch_object,
                        json_error = out.json_error,
                        hip_device_debug = out.hip_device_debug,
                        hip_arch_debug_files = hip_outs.arch_debug_files,
                    )
                )
            else:
                processed.append(out)
        return processed

    return objects

def _compiler_supports_header_units(compiler_info: typing.Any):
    return "clang" in compiler_info.compiler_type and compiler_info.supports_two_phase_compilation

def _get_module_name(target_label: Label, group_name: str) -> str:
    return paths.normalize(
        paths.join(
            "__header_units__",
            target_label.package,
            "{}{}.h".format(target_label.name, group_name),
        )
    )

def _get_import_filename(target_label: Label, group_name: str) -> str:
    return paths.normalize(
        paths.join(
            target_label.package,
            "__import__{}{}.h".format(target_label.name, group_name),
        )
    )

def _is_standalone_header(header: CHeader) -> bool:
    if header.artifact.extension not in HeaderExtension.values():
        return False
    if header.name.endswith("-inl.h") or header.name.endswith("-inl.hpp"):
        return False
    if header.name.endswith(".tcc"):
        return False
    if header.name.endswith("-pre.h") or header.name.endswith("-pre.hpp"):
        return False
    if header.name.endswith("-post.h") or header.name.endswith("-post.hpp"):
        return False
    return True

def _convert_raw_header(target_label: Label, raw_header: Artifact, include_dirs: list[CellPath]) -> CHeader:
    full_path = str(target_label.path) + "/" + raw_header.short_path
    best_dir = None
    for d in include_dirs:
        abs_dir = str(d)
        if paths.starts_with(full_path, abs_dir) and (best_dir == None or len(abs_dir) > len(best_dir)):
            best_dir = abs_dir
    if best_dir != None:
        ns = paths.dirname(paths.relativize(full_path, best_dir))
    else:
        ns = paths.dirname(raw_header.short_path)
    return CHeader(
        artifact = raw_header,
        name = raw_header.basename,
        namespace = ns,
        named = False,
    )

def _get_raw_header_include_dirs(preprocessors: list[CPreprocessor]) -> list[CellPath]:
    return flatten([x.include_dirs for x in preprocessors]) + flatten(
        [x.system_include_dirs.include_dirs for x in preprocessors if x.system_include_dirs != None]
    )

def _create_precompile_cmd(
    actions: AnalysisActions,
    target_label: Label,
    compiler_info: typing.Any,
    preprocessors: list[CPreprocessor],
    header_group: str | None,
    group_name: str,
    extra_preprocessors: list[CPreprocessor],
    cmd: CxxCompileCommand,
) -> CxxSrcPrecompileCommand:
    include_dirs = _get_raw_header_include_dirs(preprocessors)
    converted_headers = [_convert_raw_header(target_label, raw_header, include_dirs) for raw_header in flatten([x.raw_headers for x in preprocessors])]
    header_group_regex = regex(header_group, fancy = False) if header_group else None

    header_paths = [
        paths.normalize(paths.join(header.namespace, header.name))
        for header in flatten([x.headers for x in preprocessors]) + converted_headers
        if (_is_standalone_header(header) if header_group_regex == None else header_group_regex.match(header.name))
    ]

    input_header_contents = cmd_args(header_paths, format = '#include "{}"')

    module_name = _get_module_name(target_label, group_name)
    import_name = _get_import_filename(target_label, group_name)

    input_header = actions.write(
        module_name,
        cmd_args(input_header_contents, cmd_args(["\n"])),
        has_content_based_path = True,
    )

    import_stub = actions.write(
        import_name,
        """
#ifdef FACEBOOK_CPP_HEADER_UNIT
export
#endif
import \"{}\";
""".format(module_name),
        has_content_based_path = True,
    )

    symlinked_files = {}
    for path in header_paths:
        symlinked_files[path] = import_stub

    modulemap_content = """
module "{}" {{
  header "{}"
  export *
}}
""".format(module_name, module_name)
    modulemap_file = actions.write(
        "module.modulemap" + group_name,
        modulemap_content,
        has_content_based_path = True,
    )

    src_dir = actions.symlinked_dir(
        "header-unit" + group_name,
        symlinked_files
        | {
            module_name: input_header,
            import_name: import_stub,
            "module.modulemap": modulemap_file,
        },
        has_content_based_path = True,
    )

    args = []
    args.extend([
        # TODO(nml): Fix warning bugs.
        "-Wno-uninitialized",
        "-Wno-conversion",
        "-Wno-zero-as-null-pointer-constant",
        "-Wno-c++98-compat-extra-semi",
    ])

    extra_argsfile = None
    if extra_preprocessors:
        extra_argsfile = _mk_header_units_argsfile(
            actions = actions,
            compiler_info = compiler_info,
            preprocessor = cxx_merge_cpreprocessors(actions, extra_preprocessors, []),
            ext = CxxExtension(".cpp"),
            has_content_based_path = True,
            filename_prefix = "export{}_".format(group_name),
        )

    args.extend(["-xc++-user-header", "-fmodule-header"])
    args.extend(["-fmodule-name={}".format(module_name)])
    args.extend(["-Xclang", "-fmodule-file-home-is-cwd"])

    # check_args: same setup but -fsyntax-only instead of --precompile, and
    # without -DFACEBOOK_CPP_HEADER_UNIT=1 (the `export` keyword in import
    # stubs is only valid inside module purview set up by --precompile).
    check_args = list(args)

    args.extend(["-Xclang", cmd_args(input_header, format = "-fmodules-embed-file={}")])
    args.extend(["-DFACEBOOK_CPP_HEADER_UNIT=1", "--precompile", input_header])

    check_args.extend(["-fsyntax-only", input_header])

    return CxxSrcPrecompileCommand(
        src = input_header,
        include_dir = src_dir,
        cxx_compile_cmd = cmd,
        args = args,
        check_args = check_args,
        extra_argsfile = extra_argsfile,
    )

def _precompile_single_cxx(
    actions: AnalysisActions,
    target_label: Label,
    toolchain: CxxToolchainInfo,
    impl_params: CxxRuleConstructorParams,
    group_name: str,
    src_compile_cmd: CxxSrcPrecompileCommand,
) -> HeaderUnit:
    identifier = src_compile_cmd.include_dir.short_path
    cxx_cmd = src_compile_cmd.cxx_compile_cmd

    filename = "{}.pcm".format(identifier)
    module = actions.declare_output(
        "__pcm_files__",
        filename,
        has_content_based_path = True,
    )

    # Declare stub artifact
    stub_filename = "{}.pcm.stub".format(identifier)
    stub = actions.declare_output(
        "__pcm_files__",
        stub_filename,
        has_content_based_path = True,
    )

    # Build actual header units
    # TODO(nml): We don't meaningfully support dep files. See T225373444.
    cmd = cmd_args(cxx_cmd.base_compile_cmd)
    if cxx_cmd.header_units_argsfile:
        cmd.add(cxx_cmd.header_units_argsfile.cmd_form)
    if src_compile_cmd.extra_argsfile:
        cmd.add(src_compile_cmd.extra_argsfile.cmd_form)
    cmd.add(cxx_cmd.argsfile.cmd_form)
    cmd.add(src_compile_cmd.args)
    cmd.add("-o", module.as_output())

    clang_trace = None
    if toolchain.clang_trace and toolchain.cxx_compiler_info.compiler_type == "clang":
        clang_trace = actions.declare_output(
            paths.join("__pcm_files__", "{}.json".format(identifier)),
            has_content_based_path = True,
        )
        cmd.add(cmd_args("-ftime-trace", hidden = clang_trace.as_output()))

    actions.run(
        cmd,
        category = "cxx_modules_precompile",
        identifier = identifier,
        allow_cache_upload = cxx_cmd.allow_cache_upload,
        low_pass_filter = False,
    )

    # Diagnostics: run -fsyntax-only variant for [check] subtarget.
    # This is cheaper than --precompile because it skips module serialization.
    # We must remove -DFACEBOOK_CPP_HEADER_UNIT=1 because that macro gates
    # `export` keywords in import stubs, which are only valid inside module
    # purview (set up by --precompile but not -fsyntax-only).
    diagnostics = None
    if cxx_cmd.compiler_type == "clang":
        diagnostics = actions.declare_output(
            "__diagnostics__",
            "{}.header-unit.diag.txt".format(identifier),
            has_content_based_path = False,
        )
        check_cmd = cmd_args(cxx_cmd.base_compile_cmd)
        if cxx_cmd.header_units_argsfile:
            check_cmd.add(cxx_cmd.header_units_argsfile.cmd_form)
        if src_compile_cmd.extra_argsfile:
            check_cmd.add(src_compile_cmd.extra_argsfile.cmd_form)
        check_cmd.add(cxx_cmd.argsfile.cmd_form)

        # Add all precompile args except --precompile and -DFACEBOOK_CPP_HEADER_UNIT=1
        check_cmd.add(src_compile_cmd.check_args)
        actions.run(
            [
                toolchain.internal_tools.stderr_to_file,
                cmd_args(diagnostics.as_output(), format = "--out={}"),
                check_cmd,
            ],
            category = "check",
            identifier = identifier + " (header-unit)",
            allow_cache_upload = cxx_cmd.allow_cache_upload,
            low_pass_filter = False,
        )

    # Build the stub artifact using the stub_header_unit tool
    stub_cmd = cmd_args(toolchain.internal_tools.stub_header_unit)
    stub_cmd.add(cxx_cmd.base_compile_cmd)
    if cxx_cmd.header_unit_stubs_argsfile:
        stub_cmd.add(cmd_args(hidden = cxx_cmd.header_unit_stubs_argsfile.file))
    stub_cmd.add(cxx_cmd.argsfile.cmd_form)
    stub_cmd.add(src_compile_cmd.args)
    stub_cmd.add("-o", stub.as_output())

    actions.run(
        stub_cmd,
        category = "cxx_modules_precompile_stub",
        identifier = identifier,
        allow_cache_upload = cxx_cmd.allow_cache_upload,
        low_pass_filter = False,
    )

    return HeaderUnit(
        name = _get_module_name(target_label, group_name),
        module = module,
        stub = stub,
        include_dir = src_compile_cmd.include_dir,
        import_include = _get_import_filename(target_label, group_name) if impl_params.export_header_unit == "preload" else None,
        clang_trace = clang_trace,
        diagnostics = diagnostics,
    )

def precompile_cxx(
    actions: AnalysisActions,
    target_label: Label,
    toolchain: CxxToolchainInfo,
    impl_params: CxxRuleConstructorParams,
    preprocessors: list[CPreprocessor],
    header_preprocessor_info: CPreprocessorInfo,
) -> (list[CPreprocessor], list[CxxSrcPrecompileCommand]):
    """Produces header units and returns their preprocessors and commands."""
    compiler_info = toolchain.cxx_compiler_info
    if not _compiler_supports_header_units(compiler_info):
        return [], []

    def mk_base_cmd():
        base_compile_cmd = _get_compile_base(toolchain, compiler_info, use_wrapper = True)
        ext = CxxExtension(".cpp")
        headers_tag = actions.artifact_tag()  # Currently ignored
        argsfile = _mk_argsfiles(
            actions,
            target_label,
            impl_params,
            compiler_info,
            header_preprocessor_info,
            ext,
            headers_tag,
            is_xcode_argsfile = False,
            is_precompile = True,
            uses_content_based_paths = True,
            filename_prefix = "pre_",
        )
        header_units_argsfile = _mk_header_units_argsfile(
            actions,
            compiler_info,
            header_preprocessor_info,
            ext,
            has_content_based_path = True,
            filename_prefix = "pre_",
            stub = False,
        )
        header_unit_stubs_argsfile = _mk_header_units_argsfile(
            actions,
            compiler_info,
            header_preprocessor_info,
            ext,
            has_content_based_path = True,
            filename_prefix = "pre_",
            stub = True,
        )
        return CxxCompileCommand(
            base_compile_cmd = base_compile_cmd,
            argsfile = argsfile,
            xcode_argsfile = argsfile,  # Unused
            header_units_argsfile = header_units_argsfile,
            header_unit_stubs_argsfile = header_unit_stubs_argsfile,
            headers_dep_files = None,
            compiler_type = compiler_info.compiler_type,
            category = "cxx_modules_precompile",
            allow_cache_upload = impl_params.allow_cache_upload,
            allow_content_based_paths = True,  # Unused: we always use content-based paths for precompile
        )

    cmd = mk_base_cmd()

    header_unit_preprocessors = []
    commands = []
    if len(impl_params.export_header_unit_filter) <= 1:
        group = None
        if impl_params.export_header_unit_filter:
            group = impl_params.export_header_unit_filter[0]
        precompile_cmd = _create_precompile_cmd(
            actions = actions,
            target_label = target_label,
            compiler_info = toolchain.cxx_compiler_info,
            preprocessors = preprocessors,
            header_group = group,
            group_name = "",
            extra_preprocessors = [],
            cmd = cmd,
        )
        header_unit = _precompile_single_cxx(actions, target_label, toolchain, impl_params, "", precompile_cmd)
        commands.append(precompile_cmd)
        header_unit_preprocessors.append(CPreprocessor(header_units = [header_unit]))
    else:
        # Chain preprocessors in order.
        i = 0
        for header_group in impl_params.export_header_unit_filter:
            name = ".{}".format(i)
            precompile_cmd = _create_precompile_cmd(
                actions = actions,
                target_label = target_label,
                compiler_info = toolchain.cxx_compiler_info,
                preprocessors = preprocessors,
                header_group = header_group,
                group_name = name,
                extra_preprocessors = header_unit_preprocessors,
                cmd = cmd,
            )
            header_unit = _precompile_single_cxx(actions, target_label, toolchain, impl_params, name, precompile_cmd)
            commands.append(precompile_cmd)
            header_unit_preprocessors.append(CPreprocessor(header_units = [header_unit]))
            i += 1

    return header_unit_preprocessors, commands

def cxx_objects_sub_targets(outs: list[CxxCompileOutput]) -> dict[str, list[Provider]]:
    objects_sub_targets = {}
    for obj in outs:
        sub_targets = {}
        if obj.clang_trace:
            sub_targets["clang-trace"] = [DefaultInfo(obj.clang_trace)]
        if obj.clang_remarks:
            sub_targets["clang-remarks"] = [DefaultInfo(obj.clang_remarks)]
        if obj.clang_llvm_statistics:
            sub_targets["clang-llvm-statistics"] = [DefaultInfo(obj.clang_llvm_statistics)]
        if obj.assembly:
            sub_targets["assembly"] = [DefaultInfo(obj.assembly)]
        if obj.preproc:
            sub_targets["preprocessed"] = [DefaultInfo(obj.preproc)]
        if obj.dist_cuda:
            sub_targets["nvcc-dag"] = [DefaultInfo(obj.dist_cuda.nvcc_dag)]
            sub_targets["nvcc-env"] = [DefaultInfo(obj.dist_cuda.nvcc_env)]
        objects_sub_targets[obj.object.short_path] = [
            DefaultInfo(
                obj.object,
                sub_targets = sub_targets,
            )
        ]
    return objects_sub_targets

def _validate_target_headers(label: Label, preprocessor: list[CPreprocessor]):
    path_to_artifact = {}
    all_headers = flatten([x.headers for x in preprocessor])
    for header in all_headers:
        header_path = paths.join(header.namespace, header.name)
        artifact = path_to_artifact.get(header_path)
        if artifact != None:
            if artifact != header.artifact:
                fail("Conflicting headers {} and {} map to {} in target {}".format(artifact, header.artifact, header_path, label))
        else:
            path_to_artifact[header_path] = header.artifact

def _get_compiler_info(toolchain: CxxToolchainInfo, ext: CxxExtension) -> typing.Any:
    compiler_info = None
    if ext.value in (".cpp", ".cc", ".cl", ".cxx", ".c++", ".h", ".hpp", ".hh", ".h++", ".hxx", ".bc"):
        compiler_info = toolchain.cxx_compiler_info
    elif ext.value == ".c":
        compiler_info = toolchain.c_compiler_info
    elif ext.value == ".m":
        compiler_info = toolchain.objc_compiler_info
    elif ext.value == ".mm":
        compiler_info = toolchain.objcxx_compiler_info
    elif ext.value in (".s", ".sx", ".S"):
        compiler_info = toolchain.as_compiler_info
    elif ext.value == ".cu":
        compiler_info = toolchain.cuda_compiler_info
    elif ext.value == ".hip":
        compiler_info = toolchain.hip_compiler_info
    elif ext.value in (".asm", ".asmpp"):
        compiler_info = toolchain.asm_compiler_info
    else:
        # This should be unreachable as long as we handle all enum values
        fail("Unknown C++ extension: " + ext.value)

    if not compiler_info:
        fail("Could not find compiler for extension `{ext}`".format(ext = ext.value))

    return compiler_info

def _get_category(ext: CxxExtension) -> str:
    if ext.value in (".cpp", ".cc", ".cl", ".cxx", ".c++", ".h", ".hpp", ".hh", ".h++", ".hxx"):
        return "cxx_compile"
    if ext.value == ".c":
        return "c_compile"
    if ext.value == ".m":
        return "objc_compile"
    if ext.value == ".mm":
        return "objcxx_compile"
    elif ext.value in (".s", ".sx", ".S", ".asm", ".asmpp"):
        return "asm_compile"
    elif ext.value == ".cu":
        return "cuda_compile"
    elif ext.value == ".hip":
        return "hip_compile"
    elif ext.value == ".bc":
        return "bitcode_compile"
    else:
        # This should be unreachable as long as we handle all enum values
        fail("Unknown extension: " + ext.value)

def _get_compile_base(toolchain: CxxToolchainInfo, compiler_info: typing.Any, use_wrapper, compiler_override = None) -> cmd_args:
    """
    Given a compiler info returned by _get_compiler_info, form the base compile args.

    `compiler_override` substitutes the compiler executable while keeping the
    toolchain-level wrapping (e.g. cwd remapping) that all compiles of this
    compiler type must run under.
    """
    if compiler_override != None:
        compiler = compiler_override
    else:
        compiler = compiler_info.compiler_with_wrapper if compiler_info.compiler_with_wrapper and use_wrapper else compiler_info.compiler

    if toolchain.remap_cwd and compiler_info.compiler_type in ["clang", "clang_windows", "clang_cl", "gcc"]:
        return cmd_args(toolchain.internal_tools.remap_cwd, compiler)
    else:
        return cmd_args(compiler)

def _dep_file_type(ext: CxxExtension) -> [DepFileType, None]:
    # Raw assembly doesn't make sense to capture dep files for.
    # .S is preprocessed assembly, but some builds use it with
    # assemblers that don't support -MF, so leave depfiles off.
    if ext.value in (".s", ".S", ".asm"):
        return None
    elif ext.value == ".hip":
        # TODO (T118797886): HipCompilerInfo doesn't have dep files processor.
        # Should it?
        return None
    elif ext.value == ".bc":
        # Bitcode doesn't have depfiles
        return None

    # Return the file type as well
    if ext.value in (".cpp", ".cc", ".cl", ".mm", ".cxx", ".c++", ".h", ".hpp", ".hh", ".h++", ".hxx"):
        return DepFileType("cpp")
    elif ext.value in (".c", ".m"):
        return DepFileType("c")
    elif ext.value == ".cu":
        return DepFileType("cuda")
    elif ext.value in (".asmpp", ".sx"):
        return DepFileType("asm")
    else:
        # This should be unreachable as long as we handle all enum values
        fail("Unknown C++ extension: " + ext.value)

def _add_compiler_info_flags(compiler_info: typing.Any) -> list:
    cmd = []
    cmd.append(compiler_info.preprocessor_flags or [])
    cmd.append(compiler_info.compiler_flags or [])

    return cmd

def mk_toolchain_precompile_argsfile(actions: AnalysisActions, name: str, argsfile: cmd_args, filter_argsfile: RunInfo) -> cmd_args:
    """
    Copy a toolchain argsfile with flags that are invalid for C++20 module
    precompilation filtered out. One filter action per toolchain, run only if
    a precompile demands it. `argsfile` is an `argsfile_with_artifacts()`
    value; the filtered copy carries everything the original carried.
    """
    filtered = actions.declare_output(name + "_toolchain_args_precompile", has_content_based_path = True)
    actions.run(
        [filter_argsfile, _PRECOMPILE_OPTION_IGNORE_REGEX_STR, argsfile, filtered.as_output()],
        category = "filter_modules_precompile_argsfile",
        identifier = name,
    )
    return argsfile_with_artifacts(filtered, argsfile)

def compiler_info_with_toolchain_argsfiles(
    actions: AnalysisActions, name: str, ctor: typing.Callable, compiler_info: typing.Any, precompile_filter: RunInfo | None = None
) -> typing.Any:
    """
    Given a compiler_info that can have a new version made with ctor, this
    will generate new `argsfile`/`argsfile_xcode` containing the compiler_info's
    preprocessor and compiler flags. Compile commands use these per-toolchain
    files, so targets using the toolchain don't each write their own
    `{ext}.toolchain_cxx_args` copy of the same flags. With `precompile_filter`,
    also generate `argsfile_precompile`.
    """
    if compiler_info == None:
        return None
    is_nasm = compiler_info.compiler_type == "nasm"
    flags = _add_compiler_info_flags(compiler_info)
    content_based = bool(compiler_info.supports_content_based_paths)
    argsfile = _mk_argsfile(
        actions = actions,
        name = name + "_toolchain_args",
        args = create_cmd_args(is_nasm, False, flags),
        has_content_based_path = content_based,
    )
    argsfile_xcode = _mk_argsfile(
        actions = actions,
        name = name + "_toolchain_args_xcode",
        args = create_cmd_args(is_nasm, True, flags),
        has_content_based_path = content_based,
    )
    argsfile_precompile = None
    if precompile_filter != None:
        argsfile_precompile = mk_toolchain_precompile_argsfile(actions, name, argsfile, precompile_filter)
    return compiler_info_with_argsfiles(
        compiler_info,
        ctor,
        argsfile,
        argsfile_xcode,
        argsfile_precompile,
    )

def _add_compiler_type_flags(target_label: Label, compiler_type: str, ext: CxxExtension) -> list:
    cmd = []
    cmd.append(get_flags_for_reproducible_build(target_label, compiler_type))

    if ext.value not in (".asm", ".asmpp"):
        # Clang's asm compiler doesn't support colorful output, so we skip this there.
        cmd.append(get_flags_for_colorful_output(compiler_type))

    return cmd

def _compiler_type_flags_anon_impl(ctx: AnalysisContext):
    is_nasm = ctx.attrs.compiler_type == "nasm"
    args = _add_compiler_type_flags(ctx.label, ctx.attrs.compiler_type, CxxExtension(ctx.attrs.src_extension))
    content = create_cmd_args(is_nasm, ctx.attrs.is_xcode_argsfile, *args)
    argsfile_artifact, _ = ctx.actions.write("compiler_type_args", content, allow_args = True, has_content_based_path = True)

    return [DefaultInfo(default_outputs = [argsfile_artifact])]

_compiler_type_flags_anon_rule = anon_rule(
    impl = _compiler_type_flags_anon_impl,
    attrs = {
        "compiler_type": attrs.string(doc = "The compiler type. Examples: clang, gcc, nasm"),
        "is_xcode_argsfile": attrs.bool(doc = "Apply xcode specific formatting to the argsfile."),
        "src_extension": attrs.string(doc = "The extension of the source file being compiled. See `CxxExtension` enum."),
    },
    artifact_promise_mappings = {
        "argsfile": lambda x: x[DefaultInfo].default_outputs[0],
    },
    doc = "Creates compiler flags argsfile for a given compiler type. " + "The argsfile is shared between targets, thus reducing resource usage.",
)

def create_cmd_args(is_nasm: bool, is_xcode_argsfile: bool, *args) -> cmd_args:
    if is_xcode_argsfile:
        return cmd_args(replace_regex = XCODE_ARG_SUBSTITUTIONS, *args)
    elif is_nasm:
        return cmd_args(*args)
    else:
        return cmd_args(quote = "shell", *args)

_PRECOMPILE_OPTION_IGNORE_REGEX_STR = (
    "^(?:"
    + "|".join([
        # Debug flags, which only affect debug symbols in the backend.
        "-f(?:no-)?debug-types-section",
        "-g(?:no-)?pubnames",
        "-g(?:no-)?record-gcc-switches",
        "-g(?:no-)?simple-template-names",
        "-g(?:no-)?split-dwarf(?:=.*)?",
        "-g[0-3]",
        "-gdwarf.*",
        # Profiling flags, used for PGO in the backend.
        "-fdebug-info-for-profiling",
        "-fprofile-instr-use=.*",
        "-fprofile-sample-use=.*",
        "-fprofile-use=.*",
    ])
    + ")$"
)

_PRECOMPILE_OPTION_IGNORE_REGEX = regex(_PRECOMPILE_OPTION_IGNORE_REGEX_STR)

def _filter_precompile_args(args: list[typing.Any]) -> list[typing.Any]:
    def should_ignore(s):
        return _PRECOMPILE_OPTION_IGNORE_REGEX.match(repr(s)[1:-1])

    return filter(
        None,
        [_filter_precompile_args(arg) if type(arg) == type([]) else arg if not should_ignore(arg) else None for arg in args],
    )

def _cxx_flags_args(flags, ext: str, is_precompile: bool):
    if not is_precompile:
        return flags.project_as_args(
            cxx_flags_projection_name(ext),
            ordering = "postorder",
        )

    values = flags.traverse(ordering = "postorder")
    args = [
        [
            value.preprocessor_flags,
            cxx_by_language_ext(value.lang_preprocessor_flags, ext),
            cxx_by_language_ext(value.lang_compiler_flags, ext),
            value.compiler_flags,
        ]
        for value in values
    ]
    return _filter_precompile_args(args)

def _cxx_flags_argsfile_anon_impl(ctx: AnalysisContext):
    flags = cxx_flags_tset(ctx.actions, ctx.attrs.flags)
    if flags == None:
        fail("cxx flags argsfile requires at least one `cxx_flags` target")
    args = _cxx_flags_args(flags, ctx.attrs.src_extension, ctx.attrs.is_precompile)
    content = create_cmd_args(ctx.attrs.is_nasm, ctx.attrs.is_xcode_argsfile, args)
    argsfile, _ = ctx.actions.write("cxx_flags_args", content, allow_args = True, has_content_based_path = True)
    return [DefaultInfo(default_outputs = [argsfile])]

_cxx_flags_argsfile_anon_rule = anon_rule(
    impl = _cxx_flags_argsfile_anon_impl,
    attrs = {
        "flags": attrs.list(attrs.dep(providers = [CxxFlagsInfo])),
        "is_nasm": attrs.bool(),
        "is_precompile": attrs.bool(),
        "is_xcode_argsfile": attrs.bool(),
        "src_extension": attrs.string(),
    },
    artifact_promise_mappings = {
        "argsfile": lambda x: x[DefaultInfo].default_outputs[0],
    },
    doc = "Creates a shared argsfile from one or more nested `cxx_flags` targets.",
)

def _filter_precompile_argsfile_anon_impl(ctx: AnalysisContext):
    argsfile = ctx.actions.declare_output("filtered_args", has_content_based_path = True)
    ctx.actions.run(
        [
            ctx.attrs._cxx_toolchain[CxxToolchainInfo].internal_tools.filter_argsfile,
            _PRECOMPILE_OPTION_IGNORE_REGEX_STR,
            ctx.attrs.src,
            argsfile.as_output(),
        ],
        category = "filter_modules_precompile_argsfile",
        allow_cache_upload = ctx.attrs.allow_cache_upload,
    )
    return [DefaultInfo(default_outputs = [argsfile])]

_filter_precompile_argsfile_anon_rule = anon_rule(
    impl = _filter_precompile_argsfile_anon_impl,
    attrs = {
        "allow_cache_upload": attrs.bool(),
        "src": attrs.source(),
        "_cxx_toolchain": attrs.dep(),
    },
    artifact_promise_mappings = {
        "argsfile": lambda x: x[DefaultInfo].default_outputs[0],
    },
)

def _target_compile_flags(impl_params: CxxRuleConstructorParams, compiler_type: str, ext: CxxExtension) -> list[typing.Any]:
    """The target's own flags for `ext`, in command-line order."""
    return [
        # preprocessor
        impl_params.preprocessor_flags,
        cxx_by_language_ext(impl_params.lang_preprocessor_flags, ext.value),
        get_flags_for_compiler_type(compiler_type),
        # compiler
        cxx_by_language_ext(impl_params.lang_compiler_flags, ext.value),
        # ctx.attrs.compiler_flags need to come last to preserve buck1 ordering, this prevents compiler
        # flags ordering-dependent build errors
        impl_params.compiler_flags,
    ]

def _mk_argsfiles(
    actions: AnalysisActions,
    target_label: Label,
    impl_params: CxxRuleConstructorParams,
    compiler_info: typing.Any,
    preprocessor: CPreprocessorInfo,
    ext: CxxExtension,
    headers_tag: ArtifactTag,
    is_xcode_argsfile: bool,
    is_precompile: bool = False,
    filename_prefix: str = "",
    uses_content_based_paths: bool = False,
) -> CompileArgsfile:
    """
    Generate and return an {ext}.argsfile artifact and command args that utilize the argsfile.
    """
    is_nasm = compiler_info.compiler_type == "nasm"

    # prefix example: .cpp.xcode_
    filename_prefix = "{src_file_extension}.{xcode_prefix}{filename_prefix}".format(
        src_file_extension = ext.value,
        xcode_prefix = "xcode_" if is_xcode_argsfile else "",
        filename_prefix = filename_prefix,
    )

    argsfiles = []
    args_list = []

    # The arguments by contributing layer, for `CompileArgsfile.layers` (Xcode argsfiles alone).
    layers = {}

    def add_layer(name: str, args):
        if not is_xcode_argsfile:
            return
        if name in layers:
            fail("compile flags layer `{}` added twice for `{}`; each layer of the argsfile is recorded once".format(name, ext.value))
        layers[name] = create_cmd_args(is_nasm, is_xcode_argsfile, args)

    def mk_argsfile(filename: str, args, absolute: bool = False, use_dep_files_placeholder_for_content_based_paths: bool = False) -> Artifact:
        content = create_cmd_args(is_nasm, is_xcode_argsfile, args)
        argsfile, _ = actions.write(
            filename,
            content,
            allow_args = True,
            absolute = absolute,
            has_content_based_path = uses_content_based_paths,
            use_dep_files_placeholder_for_content_based_paths = use_dep_files_placeholder_for_content_based_paths,
        )
        return argsfile

    def write_for_dep_file_filtering(filename: str, content, absolute: bool = False) -> (Artifact, Artifact | None):
        # Dep-file filtering needs two versions of the file:
        # - The first is a tagged file with the optionally content-based paths, which
        #   will be passed to the compiler as usual, but will NOT be marked used, so
        #   whether the paths in it change or not, the action will NOT rerun.
        # - The second is an untagged file with the same content except content-based
        #   paths are replaced by placeholders. This file is not read by the compiler,
        #   but is passed as a hidden input of the compiler action, so that if
        #   something OTHER than paths in the file changes, this redacted version will
        #   change and trigger a rerun.
        output, _ = actions.write(
            filename,
            content,
            allow_args = True,
            absolute = absolute,
            has_content_based_path = uses_content_based_paths,
        )
        # Precompile actions do not filter inputs with dep files, so the real
        # argsfile already tracks flag changes.
        if is_precompile:
            return (output, None)

        redacted_output, _ = actions.write(
            filename + "_redacted",
            content,
            allow_args = True,
            absolute = absolute,
            has_content_based_path = uses_content_based_paths,
            use_dep_files_placeholder_for_content_based_paths = True,
        )
        return (output, redacted_output)

    def mk_argsfile_for_dep_file_filtering(filename: str, args, absolute: bool = False) -> (Artifact, Artifact | None):
        return write_for_dep_file_filtering(
            filename,
            create_cmd_args(is_nasm, is_xcode_argsfile, args),
            absolute = absolute,
        )

    def make_toolchain_argsfile():
        compiler_info_flags = _add_compiler_info_flags(compiler_info)

        if is_precompile:
            if impl_params._cxx_toolchain == None:
                # TODO(nml): Compared to get_cxx_toolchain_info(), we don't support
                # AppleToolchain for C++20 modules. Update this if that changes.
                fail("C++20 modules are not supported for AppleToolchain")

            if compiler_info.argsfile_precompile and not is_xcode_argsfile:
                compiler_info_argsfile = compiler_info.argsfile_precompile
            else:
                # The anon filter takes an `attrs.source()` argsfile, so write
                # a per-target copy of the flags for it.
                unfiltered_argsfile = mk_argsfile(filename_prefix + "toolchain_cxx_args", compiler_info_flags)
                filtered_info_argsfile = actions.anon_target(
                    _filter_precompile_argsfile_anon_rule,
                    {
                        "allow_cache_upload": impl_params.allow_cache_upload,
                        "src": unfiltered_argsfile,
                        "_cxx_toolchain": impl_params._cxx_toolchain,
                    },
                ).artifact("argsfile")

                # The promise artifact doesn't carry the artifacts the flags
                # reference forward automatically for us; re-pair them.
                compiler_info_argsfile = argsfile_with_artifacts(
                    actions.assert_has_content_based_path(filtered_info_argsfile),
                    compiler_info_flags,
                )
        elif compiler_info.argsfile and not is_xcode_argsfile:
            compiler_info_argsfile = compiler_info.argsfile
        elif compiler_info.argsfile_xcode and is_xcode_argsfile:
            compiler_info_argsfile = compiler_info.argsfile_xcode
        else:
            # filename example: .cpp.toolchain_cxx_args
            compiler_info_filename = filename_prefix + "toolchain_cxx_args"
            compiler_info_argsfile = mk_argsfile(compiler_info_filename, compiler_info_flags)

        argsfiles.append(compiler_info_argsfile)
        args_list.append(compiler_info_flags)
        add_layer("toolchain", compiler_info_flags)

    make_toolchain_argsfile()

    def make_compiler_type_argsfile():
        compiler_type_flags = _add_compiler_type_flags(target_label, compiler_info.compiler_type, ext)

        if impl_params.anon_targets_allowed:
            compiler_type_flags_anon_target = actions.anon_target(
                _compiler_type_flags_anon_rule,
                {
                    "compiler_type": compiler_info.compiler_type,
                    "is_xcode_argsfile": is_xcode_argsfile,
                    "src_extension": ext.value,
                },
            )
            compiler_type_argsfile_artifact = actions.assert_has_content_based_path(compiler_type_flags_anon_target.artifact("argsfile"))
        else:
            compiler_type_argsfile_artifact = mk_argsfile(
                filename_prefix + "compiler_type_args",
                compiler_type_flags,
            )
        argsfiles.append(compiler_type_argsfile_artifact)
        args_list.append(compiler_type_flags)
        add_layer("compiler_type", compiler_type_flags)

    make_compiler_type_argsfile()

    def make_cxx_flags_argsfile():
        flag_deps = impl_params.cxx_flags
        if not flag_deps:
            add_layer("cxx_flags", [])
            return

        flags = cxx_flags_tset(actions, flag_deps)
        projection = flags.project_as_args(
            cxx_flags_projection_name(ext.value),
            ordering = "postorder",
        )
        if impl_params.anon_targets_allowed:
            argsfile = actions.anon_target(
                _cxx_flags_argsfile_anon_rule,
                {
                    "flags": flag_deps,
                    "is_nasm": is_nasm,
                    "is_precompile": is_precompile,
                    "is_xcode_argsfile": is_xcode_argsfile,
                    "src_extension": ext.value,
                },
            ).artifact("argsfile")
            argsfile = actions.assert_has_content_based_path(argsfile)
        else:
            argsfile = mk_argsfile(
                filename_prefix + "cxx_flags_args",
                _cxx_flags_args(flags, ext.value, is_precompile),
            )

        argsfiles.append(argsfile_with_artifacts(argsfile, projection))
        args_list.append(projection)
        add_layer("cxx_flags", projection)

    make_cxx_flags_argsfile()

    def make_deps_argsfile():
        deps_args = []
        if is_precompile:
            # TODO(nml): We don't support dep files for now in precompile.
            deps_args.append(preprocessor.set.project_as_args("precompile_args"))
        else:
            deps_args.append(headers_tag.tag_artifacts(preprocessor.set.project_as_args("args")))

        # Different preprocessors will contain whether to use modules,
        # and the modulemap to use, so we need to get the final outcome.
        if preprocessor.set.reduce("uses_modules"):
            deps_args.append(headers_tag.tag_artifacts(preprocessor.set.project_as_args("modular_args")))

        # filename example: .cpp.deps_cxx_args, .cpp.deps_cxx_args_redacted
        deps_argsfile_for_compiler, deps_argsfile_for_buck_action_rerun = mk_argsfile_for_dep_file_filtering(filename_prefix + "deps_cxx_args", deps_args)
        argsfiles.append(
            cmd_args(
                headers_tag.tag_artifacts(deps_argsfile_for_compiler),
                hidden = [deps_argsfile_for_buck_action_rerun] if deps_argsfile_for_buck_action_rerun != None else [],
            )
        )
        args_list.extend(deps_args)
        add_layer("deps", deps_args)

    make_deps_argsfile()

    def make_target_argsfile():
        target_args = _target_compile_flags(impl_params, compiler_info.compiler_type, ext) + [
            headers_tag.tag_artifacts(preprocessor.set.project_as_args("include_dirs")),
        ]
        if is_precompile:
            target_args = _filter_precompile_args(target_args)
        target_args = cmd_args(target_args)

        # Workaround as that's not precompiled, but working just as prefix header.
        # Another thing is that it's clang specific, should be generalized.
        if impl_params.precompiled_header != None and not impl_params.precompiled_header[CPrecompiledHeaderInfo].compiled:
            target_args.add(["-include", headers_tag.tag_artifacts(impl_params.precompiled_header[CPrecompiledHeaderInfo].header)])
        if impl_params.prefix_header != None:
            target_args.add(["-include", headers_tag.tag_artifacts(impl_params.prefix_header)])

        # filename example: .cpp.target_cxx_args
        target_argsfile_filename = filename_prefix + "target_cxx_args"
        argsfiles.append(mk_argsfile(target_argsfile_filename, target_args))
        args_list.append(target_args)
        add_layer("target", target_args)

    make_target_argsfile()

    # Create a copy of the args so that we can continue to modify it later.
    args_without_file_prefix_args = cmd_args(args_list)

    # Holder, because the closure below cannot bind an enclosing name.
    file_prefix_specs = []

    def make_file_prefix_argsfile():
        if is_precompile:
            # The precompile_args field overrides these.
            return

        if compiler_info.compiler_type in ("nasm", "windows_ml64"):
            # NASM/MASM don't understand -ffile-prefix-map. .s/.sx/.S go through the gcc/clang
            # driver (as_compiler_info) and do accept these flags.
            return

        if not preprocessor.set.reduce("has_file_prefix_args"):
            return

        # Put file_prefix_args in argsfile, make sure they do not appear when evaluating $(cxxppflags)
        # to avoid "argument too long" errors
        file_prefix_args = headers_tag.tag_artifacts(preprocessor.set.project_as_args("file_prefix_args"))

        if ext.value == ".cu":
            # NVCC passes argsfile flags to device sub-tools (cicc, ptxas) which don't
            # understand -ffile-prefix-map.
            file_prefix_args = cmd_args(file_prefix_args, replace_regex = ("-ffile-prefix-map=", "-fdebug-prefix-map="))
        elif compiler_info.compiler_type == "clang":
            # -fcoverage-prefix-map is only supported by clang. Include coverage prefix args
            # only for clang to avoid breaking non-clang compilers that receive flags from
            # dependencies via the preprocessor set.
            coverage_prefix_args = headers_tag.tag_artifacts(preprocessor.set.project_as_args("coverage_prefix_args"))
            file_prefix_args = cmd_args(file_prefix_args, coverage_prefix_args)

        file_prefix_args_filename = filename_prefix + "file_prefix_cxx_args"

        # GCC copies all flags into COLLECT_GCC_OPTIONS env var before exec'ing cc1. This single
        # string can exceed Linux's 128KB MAX_ARG_STRLEN limit when there are hundreds of prefix-map
        # flags.
        #
        # Delivering via -specs injects flags directly into cc1 without touching
        # COLLECT_GCC_OPTIONS.
        #
        # Assembly may report compiler_type=="gcc" via as_compiler_info even on Clang toolchains;
        # -specs= is GCC-driver-only so skip it for assembly.
        if compiler_info.compiler_type == "gcc" and _get_category(ext) != "asm_compile":
            specs_content = cmd_args(
                "*cc1:\n+ ",
                file_prefix_args,
                delimiter = " ",
            )
            specs_file, specs_file_for_buck_action_rerun = write_for_dep_file_filtering(
                file_prefix_args_filename + ".specs",
                specs_content,
            )
            prefix_ref = cmd_args(
                headers_tag.tag_artifacts(specs_file),
                format = "-specs={}",
                hidden = specs_file_for_buck_action_rerun,
            )
            file_prefix_specs.append(specs_file)
        else:
            prefix_ref = file_prefix_args

        file_prefix_argsfile_for_compiler, file_prefix_argsfile_for_buck_action_rerun = mk_argsfile_for_dep_file_filtering(
            file_prefix_args_filename,
            prefix_ref,
        )
        argsfiles.append(
            cmd_args(
                headers_tag.tag_artifacts(file_prefix_argsfile_for_compiler),
                hidden = file_prefix_argsfile_for_buck_action_rerun,
            )
        )
        args_list.append(prefix_ref)

    make_file_prefix_argsfile()

    if is_xcode_argsfile:
        file_args = cmd_args(argsfiles, format = "@{}")
    elif is_nasm:
        file_args = cmd_args(argsfiles, format = "-@{}")
    else:
        file_args = cmd_args(argsfiles, format = "@{}", quote = "shell")

    # filename example: .cpp.cxx_compile_argsfile
    file_name = filename_prefix + "cxx_compile_argsfile"

    # For Xcode to parse argsfiles of argsfiles, the paths in the former must be absolute.
    argsfile, argsfile_for_buck_action_rerun = write_for_dep_file_filtering(
        file_name,
        file_args,
        absolute = is_xcode_argsfile,
    )

    args = create_cmd_args(is_nasm, is_xcode_argsfile, args_list)
    input_args = [args, file_args]
    if argsfile_for_buck_action_rerun != None:
        input_args.append(argsfile_for_buck_action_rerun)

    argsfile_format = "-@{}" if is_nasm else "@{}"
    cmd_form = cmd_args(
        headers_tag.tag_artifacts(argsfile),
        format = argsfile_format,
        hidden = input_args,
    )

    # The NVCC dry run expands the flags and reads header maps, nothing else behind them.
    cmd_form_cuda_dryrun = None
    if ext.value == ".cu":
        cmd_form_cuda_dryrun = cmd_args(
            argsfile,
            format = argsfile_format,
            hidden = [file_args, preprocessor.set.project_as_args("header_maps")],
        )
    return CompileArgsfile(
        file = argsfile,
        cmd_form = cmd_form,
        cmd_form_cuda_dryrun = cmd_form_cuda_dryrun,
        args = args,
        args_without_file_prefix_args = args_without_file_prefix_args,
        file_prefix_specs = file_prefix_specs[0] if file_prefix_specs else None,
        layers = layers if is_xcode_argsfile else None,
    )

def _mk_header_units_argsfile(
    actions: AnalysisActions,
    compiler_info: typing.Any,
    preprocessor: CPreprocessorInfo,
    ext: CxxExtension,
    filename_prefix: str = "",
    has_content_based_path: bool = False,
    stub: bool = False,
    headers_tag: ArtifactTag | None = None,
) -> CompileArgsfile | None:
    """
    Generate and return an argsfile artifact containing all header unit options, and
    command args that utilize the argsfile.
    """
    if not preprocessor.set:
        return None
    if _get_category(ext) != "cxx_compile":
        return None
    if not _compiler_supports_header_units(compiler_info):
        return None
    if not preprocessor.set.reduce("has_header_units_args"):
        return None

    file_name = "{}.{}header_unit{}_args".format(ext.value, filename_prefix, "_stubs" if stub else "s")
    args = cmd_args()
    args.add([
        # TODO(nml): We only support Clang 17+, which don't need/want the extra -f
        # arguments when compiling C++20. Clang 15 is too buggy to work properly, but if
        # you wanted to try, you would need the below options at the very least, to get
        # started:
        # "-fmodules",
        # "-fno-implicit-modules",
        # "-fno-implicit-module-maps",
        "-Wno-experimental-header-units",
        "-Wno-ambiguous-macro",
    ])

    projection_name = "header_unit_stubs_args" if stub else "header_units_args"
    args.add(preprocessor.set.project_as_args(projection_name))
    if headers_tag == None:
        file_args = cmd_args(args, quote = "shell")
        argsfile, _ = actions.write(
            file_name,
            file_args,
            allow_args = True,
            has_content_based_path = has_content_based_path,
        )
        cmd_form = cmd_args(argsfile, format = "@{}", hidden = file_args)
    else:
        # We write two versions of the file:
        # - The first is a tagged file with the optionally content-based paths, which
        #   will be passed to the compiler as usual, but will NOT be marked used, so
        #   whether the paths in it change or not, the action will NOT rerun.
        # - The second is an untagged file with the same content except content-based
        #   paths are replaced by placeholders. This file is not read by the compiler,
        #   but is passed as a hidden input of the compiler action, so that if
        #   something OTHER than paths in the file changes, this redacted version will
        #   change and trigger a rerun.
        # TODO(jtbraun): Unify with `write_for_dep_file_filtering` in `_mk_argsfiles`.
        file_args = cmd_args(args, quote = "shell")
        argsfile, _ = actions.write(
            file_name,
            file_args,
            allow_args = True,
            has_content_based_path = has_content_based_path,
        )
        redacted_argsfile, _ = actions.write(
            file_name + "_redacted",
            file_args,
            allow_args = True,
            has_content_based_path = has_content_based_path,
            use_dep_files_placeholder_for_content_based_paths = True,
        )
        cmd_form = cmd_args(
            headers_tag.tag_artifacts(argsfile),
            format = "@{}",
            hidden = [headers_tag.tag_artifacts(file_args), redacted_argsfile],
        )

    return CompileArgsfile(
        file = argsfile,
        cmd_form = cmd_form,
        args = file_args,
        args_without_file_prefix_args = args,
    )

def _get_dep_tracking_mode(toolchain: Provider, file_type: DepFileType) -> DepTrackingMode:
    if file_type == DepFileType("cpp") or file_type == DepFileType("c"):
        return toolchain.cpp_dep_tracking_mode
    elif file_type == DepFileType("cuda"):
        return toolchain.cuda_dep_tracking_mode
    else:
        return DepTrackingMode("makefile")

def get_compiler_type(ctx: AnalysisContext, ext: CxxExtension) -> typing.Any:
    toolchain = get_cxx_toolchain_info(ctx)
    compiler_info = _get_compiler_info(toolchain, ext)
    return compiler_info.compiler_type

def _generate_base_compile_command(
    actions: AnalysisActions,
    target_label: Label,
    toolchain: CxxToolchainInfo,
    impl_params: CxxRuleConstructorParams,
    pre: CPreprocessorInfo,
    headers_tag: ArtifactTag,
    ext: CxxExtension,
    filename_prefix: str = "",
) -> CxxCompileCommand:
    """
    Generate a common part of a compile command that is shared by all sources
    with a given extension.
    """
    compiler_info = _get_compiler_info(toolchain, ext)

    base_compile_cmd = _get_compile_base(toolchain, compiler_info, impl_params.use_fbcc_rust_wrapper)
    category = _get_category(ext)

    headers_dep_files = None
    dep_file_file_type_hint = _dep_file_type(ext)
    if dep_file_file_type_hint != None and toolchain.use_dep_files:
        tracking_mode = _get_dep_tracking_mode(toolchain, dep_file_file_type_hint)
        mk_dep_files_flags = get_headers_dep_files_flags_factory(tracking_mode)
        if mk_dep_files_flags:
            headers_dep_files = HeadersDepFiles(
                processor = cmd_args(toolchain.internal_tools.dep_file_processor),
                mk_flags = mk_dep_files_flags,
                tag = headers_tag,
                dep_tracking_mode = tracking_mode,
            )

    def gen_argsfiles(is_xcode_argsfile):
        return _mk_argsfiles(
            actions,
            target_label,
            impl_params,
            compiler_info,
            pre,
            ext,
            headers_tag,
            is_precompile = False,
            filename_prefix = filename_prefix,
            uses_content_based_paths = not is_xcode_argsfile,
            is_xcode_argsfile = is_xcode_argsfile,
        )

    argsfile = gen_argsfiles(is_xcode_argsfile = False)

    # The Xcode argsfile is consumed only by the `xcode-argsfiles` sub-target and
    # `XcodeDataInfo`, both of which are Xcode/IDE features gated on `xcode_data`.
    # Skip generating it (per source extension, per target) otherwise, saving
    # memory on non-IDE builds.
    if impl_params.generate_sub_targets.xcode_data:
        xcode_argsfile = gen_argsfiles(is_xcode_argsfile = True)
    else:
        xcode_argsfile = None

    header_units_argsfile = _mk_header_units_argsfile(
        actions,
        compiler_info,
        pre,
        ext,
        filename_prefix,
        has_content_based_path = True,
        stub = False,
        headers_tag = headers_tag,
    )

    header_unit_stubs_argsfile = _mk_header_units_argsfile(
        actions,
        compiler_info,
        pre,
        ext,
        filename_prefix,
        has_content_based_path = True,
        stub = True,
    )

    allow_content_based_paths = bool(compiler_info.supports_content_based_paths and impl_params.use_content_based_paths)
    return CxxCompileCommand(
        base_compile_cmd = base_compile_cmd,
        argsfile = argsfile,
        xcode_argsfile = xcode_argsfile,
        header_units_argsfile = header_units_argsfile,
        header_unit_stubs_argsfile = header_unit_stubs_argsfile,
        headers_dep_files = headers_dep_files,
        compiler_type = compiler_info.compiler_type,
        category = category,
        allow_cache_upload = impl_params.allow_cache_upload,
        allow_content_based_paths = allow_content_based_paths,
    )

def _get_use_pch_args(
    src: Artifact,
    cxx_compile_cmd: CxxCompileCommand,
    compile_with_pch: CPrecompiledHeaderInfo,
    precompiled_header: CPrecompiledHeaderInfo,
) -> cmd_args:
    if precompiled_header.clanguage != src.extension:
        return cmd_args()

    pch_args = cmd_args()
    if cxx_compile_cmd.compiler_type in ["windows"]:
        pch_args.add(cmd_args(compile_with_pch.basename, format = "/Yu{}"))
        pch_args.add(cmd_args(compile_with_pch.basename, format = "/FI{}"))
        pch_args.add(cmd_args(compile_with_pch.header, format = "/Fp{}"))
    elif cxx_compile_cmd.compiler_type in ["clang"]:
        pch_args.add("-Xclang", "-include-pch", "-Xclang", compile_with_pch.header)
    else:
        fail(
            "Warning: Unsupported compiler type for precompiled header usage: {}".format(
                cxx_compile_cmd.compiler_type,
            )
        )

    return pch_args

def _declare_clang_trace_output(
    toolchain: CxxToolchainInfo, compiler_type: str, actions: AnalysisActions, filename_base: str, content_based: bool
) -> Artifact | None:
    """
    Declares an output artifact for clang trace JSON file.
    """
    if toolchain.clang_trace and compiler_type == "clang":
        return actions.declare_output(
            paths.join("__objects__", "{}.json".format(filename_base)),
            has_content_based_path = content_based,
        )
    return None

def _declare_assembly(actions: AnalysisActions, compiler_type: str, object_format: CxxObjectFormat, filename_base: str, content_based: bool) -> Artifact | None:
    """
    Declares an output artifact for assembly or LLVM IR output file.
    Returns None for compilers that don't support the -S flag.
    """
    if compiler_type not in ["clang", "gcc"]:
        return None

    # Generate assembler or llvm bitcode output file
    assembly_extension = "s"
    if compiler_type == "clang" and object_format == CxxObjectFormat("bitcode"):
        assembly_extension = "ll"

    return actions.declare_output(
        "__assembly__",
        "{}.{}".format(filename_base, assembly_extension),
        has_content_based_path = content_based,
    )
