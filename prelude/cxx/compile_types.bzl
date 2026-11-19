# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo")
load(":argsfiles.bzl", "CompileArgsfile", "CompileArgsfiles")
load(":cxx_toolchain_types.bzl", "CxxObjectFormat", "DepTrackingMode")

# Supported assembly extensions
AsmExtensions = enum(
    ".s",
    ".sx",
    ".S",
    ".asm",
    ".asmpp",
)

# Supported Cxx file extensions
CxxExtension = enum(".cpp", ".cc", ".cl", ".cxx", ".c++", ".c", ".m", ".mm", ".cu", ".hip", ".h", ".hpp", ".hh", ".h++", ".hxx", ".bc", *AsmExtensions.values())

# Header files included in compilation databases
HeaderExtension = enum(
    ".h",
    ".hpp",
    ".hh",
    ".h++",
    ".hxx",
    ".cuh",
)

# Mode for using header units in compilation
UseHeaderUnitsMode = enum(
    # Don't use header units
    "none",
    # Use full precompiled modules (.pcm files)
    "pcm",
    # Use module stubs (.pcm.stub files): those are not directly used in the compilation
    # command line; instead, they are inert stand-ins that behave like real PCM files,
    # cache-wise (i.e. expected to be cached when real PCM files would be cached, and
    # rebuilt when PCM files would be). You can enable these to trace and model the
    # behaviour of module-enabled builds before actually migrating to them. This steps
    # helps estimate the overhead of precompiling modules in terms of additional
    # uncached Buck actions.
    "stub",
)

# File types for dep files
DepFileType = enum(
    "cpp",
    "c",
    "cuda",
    "asm",
)

HeadersDepFiles = record(
    # An executable to wrap the actual command with for post-processing of dep
    # files into the format that Buck2 recognizes (i.e. one artifact per line).
    processor = field(cmd_args),
    # The tag that was added to headers.
    tag = field(ArtifactTag),
    # A function that produces new cmd_args to append to the compile command to
    # get it to emit the dep file. This will receive the output dep file as an
    # input.
    mk_flags = field(typing.Callable),
    # Dependency tracking mode to know how to generate dep file
    dep_tracking_mode = field(DepTrackingMode),
)

# Output from CUDA distributed compilation
CudaDistributedCompileOutput = record(
    # Dependency graph of the NVCC sub-commands.
    nvcc_dag = field(Artifact),
    # Environment variables for the NVCC sub-commands.
    nvcc_env = field(Artifact),
    # Argsfile for the host compiler.
    hostcc_argsfile = field(Artifact),
)

# Information about how to compile a source file of particular extension.
CxxCompileCommand = record(
    # The compiler and any args which are independent of the rule.
    base_compile_cmd = field(cmd_args),
    # The argsfile of arguments from the rule and it's dependencies.
    argsfile = field(CompileArgsfile),
    # The argsfile to use for Xcode integration. Requires xcode_data subtarget.
    xcode_argsfile = field(CompileArgsfile | None),
    # The argsfile containing header units args.
    header_units_argsfile = field(CompileArgsfile | None),
    # The argsfile containing header unit stubs args.
    header_unit_stubs_argsfile = field(CompileArgsfile | None),
    headers_dep_files = field([HeadersDepFiles, None]),
    compiler_type = field(str),
    # The action category
    category = field(str),
    allow_cache_upload = field(bool),
    allow_content_based_paths = field(bool),
)

# Declared index store output and metadata
DeclaredIndexStore = record(
    output = field(Artifact),
    filename_base = field(str),
)

# Factory for creating and compiling index stores
# - declare: Called during analysis to declare the output artifact
# - compile: Called during execution (inside dynamic action) to run the compilation
IndexStoreFactory = record(
    declare = field(typing.Callable[[AnalysisActions, typing.Any], DeclaredIndexStore | None]),
    compile = field(typing.Callable[[AnalysisActions, Label, OutputArtifact, str, CxxToolchainInfo, cmd_args], None]),
)

# Information about how to compile a source file.
CxxSrcCompileCommand = record(
    # Source file to compile.
    src = field(Artifact),
    # If we have multiple source entries with same files but different flags,
    # specify an index so we can differentiate them. Otherwise, use None.
    index = field([int, None], None),
    # The CxxCompileCommand to use to compile this file.
    cxx_compile_cmd = field(CxxCompileCommand),
    # Arguments specific to the source file.
    args = field(list[typing.Any]),
    # Is this a header file?
    is_header = field(bool, False),
    # Whether to use content-based paths for the outputs of the compilation command.
    uses_content_based_paths = field(bool),
    # The index store factory to use to generate index store for this source file.
    index_store_factory = field(IndexStoreFactory | None, None),
    error_handler = field([typing.Callable, None], None),
    # Whether to set expect_eligible_for_dedupe on compile actions.
    expect_eligible_for_dedupe = field(bool, False),
    # Local executor slots the compile action takes.
    weight = field(int),
)

CxxSrcPrecompileCommand = record(
    # Generated umbrella header used as the precompile input.
    src = field(Artifact),
    # Preserve the include tree for PCM output and action naming.
    include_dir = field(Artifact),
    # The CxxCompileCommand to use to compile this file.
    cxx_compile_cmd = field(CxxCompileCommand),
    # Arguments specific to the source file.
    args = field(list[typing.Any]),
    # Arguments for -fsyntax-only check (args without --precompile and
    # -DFACEBOOK_CPP_HEADER_UNIT=1, with -fsyntax-only instead).
    check_args = field(list[typing.Any], []),
    # Extra argsfile to include after any other header units argsfile but before the
    # main argsfiles.
    extra_argsfile = field([CompileArgsfile, None], None),
)

# Output of creating compile commands for Cxx source files.
CxxCompileCommandOutput = record(
    # List of compile commands for each source file.
    src_compile_cmds = field(list[CxxSrcCompileCommand], default = []),
    # Base compile commands for each source file extension.
    base_compile_cmds = field(dict[CxxExtension, CxxCompileCommand], default = {}),
    # Argsfiles generated for compiling these source files.
    argsfiles = field(CompileArgsfiles, default = CompileArgsfiles()),
    # List of compile commands for use in compilation database generation.
    comp_db_compile_cmds = field(list[CxxSrcCompileCommand], default = []),
)

CxxCompileOutput = record(
    # The compiled `.o` file.
    object = field(Artifact),
    object_format = field(CxxObjectFormat, CxxObjectFormat("native")),
    object_has_external_debug_info = field(bool, False),
    # Externally referenced debug info, which doesn't get linked with the
    # object (e.g. the above `.o` when using `-gsplit-dwarf=single` or the
    # the `.dwo` when using `-gsplit-dwarf=split`).
    external_debug_info = field(Artifact | None, None),
    clang_remarks = field(Artifact | None, None),
    clang_llvm_statistics = field(Artifact | None, None),
    clang_trace = field(Artifact | None, None),
    gcno_file = field(Artifact | None, None),
    index_store = field(Artifact | None, None),
    assembly = field(Artifact | None, None),
    diagnostics = field(Artifact | None, None),
    clang_tidy_diagnostics = field(Artifact | None, None),
    preproc = field(Artifact | None, None),
    dist_cuda = field(CudaDistributedCompileOutput | None, None),
    pch_object = field(Artifact | None, None),
    json_error = field(Artifact | None, None),
    # True for HIP TUs under the device-debug-extract modifier.
    hip_device_debug = field(bool, False),
    # Per-arch sidecar .debug files from hip_debug_extract_compile_object.
    hip_arch_debug_files = field(dict[str, Artifact], {}),
)

CxxCompileFlavor = enum(
    # Produces position independent compile outputs
    "pic",
    # Produces position independent compile outputs
    # using optimization flags from toolchain
    "optimized",
    # Produces position independent compile outputs
    # using debug flags from toolchain
    "debug",
)
