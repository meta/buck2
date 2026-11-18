# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//cxx:cxx_apple_linker_flags.bzl",
    "apple_extra_darwin_linker_flags",
    "apple_format_target_triple",
    "apple_target_triple_flags",
    "is_valid_apple_platform_name",
)
load("@prelude//cxx:debug.bzl", "SplitDebugMode")
load("@prelude//target_stats:target_stats_tools.bzl", "TargetStatsToolsInfo")

CXX_COMPILER_TYPES = ["clang", "clang_cl", "clang_windows", "gcc", "windows", "windows_ml64"]

LinkerType = enum("gnu", "darwin", "windows", "wasm")

IncrementalLinkingMode = enum("disabled", "supported", "enabled")

ShlibInterfacesMode = enum(
    "disabled",
    "defined_only",  # Generate a "stub" shared library by only linking object files passed to the link, ignoring static libraries or dynamic libraries linked against.
    # This known to be incorrect in the presence of static libraries, as they won't be represented in the interface.
    "stub_from_library",  # Generate an interface from the completed shared library via some external tool.
    "stub_from_linker_invocation",  # For linkers that support it, generate an interface from the linker invocation that would ordinarily produce the shared library, adding some extra flags
)

# TODO(T110378149): Consider whether it makes sense to move these things to
# configurations/constraints rather than part of the toolchain.
LinkerInfo = provider(
    # @unsorted-dict-items
    fields = {
        "archiver": provider_field(typing.Any, default = None),
        "archiver_flags": provider_field(typing.Any, default = None),
        "archiver_reads_inputs": provider_field(bool, default = True),
        "archiver_supports_argfiles": provider_field(typing.Any, default = None),
        "archiver_type": provider_field(typing.Any, default = None),
        "archive_contents": provider_field(typing.Any, default = "normal"),
        "archive_objects_locally": provider_field(typing.Any, default = None),
        "archive_symbol_table": provider_field(bool, default = True),
        # "archiver_platform",
        # "" on Unix, "exe" on Windows
        "binary_extension": provider_field(typing.Any, default = None),  # str
        "dist_thin_lto_codegen_flags": provider_field([cmd_args, None], default = None),
        "extra_outputs": provider_field(list[str], default = []),
        "generate_linker_maps": provider_field(typing.Any, default = None),  # bool
        "generate_gc_sections": provider_field(typing.Any, default = None),  # bool
        # Whether to run native links locally.  We support this for fbcode platforms
        # to avoid issues with C++ static links (see comment in
        # `platform/cxx_toolchains.bzl` for details).
        "link_binaries_locally": provider_field(typing.Any, default = None),
        # Whether to run native shared library links locally. For certain use cases
        # (e.g., large Apple frameworks), it's more efficient to link locally due
        # GiBs of object files (which can also lead to RE errors/timesouts etc).
        "link_libraries_locally": provider_field(typing.Any, default = None),
        "link_style": provider_field(typing.Any, default = None),  # LinkStyle
        "link_weight": provider_field(int, default = 1),  # int
        "link_ordering": provider_field(typing.Any, default = None),  # LinkOrdering
        "linker": provider_field(typing.Any, default = None),
        "linker_flags": provider_field(typing.Any, default = None),
        "executable_linker_flags": provider_field(typing.Any, default = []),
        "binary_linker_flags": provider_field(typing.Any, default = []),
        "link_metadata_flag": provider_field(str | None, default = None),
        "lto_mode": provider_field(typing.Any, default = None),
        "mk_shlib_intf": provider_field(typing.Any, default = None),
        # "o" on Unix, "obj" on Windows
        "object_file_extension": provider_field(typing.Any, default = None),  # str
        "post_linker_flags": provider_field(typing.Any, default = None),
        # `LinkInfo`s (list[LinkInfo]) of the runtime libraries the toolchain provides.
        "runtime_library_files": provider_field(list[typing.Any], default = []),
        "sanitizer_runtime_enabled": provider_field(bool, default = False),
        "sanitizer_runtime_files": provider_field(list[Artifact], default = []),
        "shlib_interfaces": provider_field(ShlibInterfacesMode),
        "shared_dep_runtime_ld_flags": provider_field(typing.Any, default = None),
        # "lib" on Linux/Mac/Android, "" on Windows.
        "shared_library_name_default_prefix": provider_field(typing.Any, default = None),  # str
        # "{}.so" on Linux, "{}.dylib" on Mac, "{}.dll" on Windows
        "shared_library_name_format": provider_field(typing.Any, default = None),  # str
        "shared_library_versioned_name_format": provider_field(typing.Any, default = None),  # str
        "static_dep_runtime_ld_flags": provider_field(typing.Any, default = None),
        # "a" on Unix, "lib" on Windows
        "static_library_extension": provider_field(typing.Any, default = None),  # str
        "static_pic_dep_runtime_ld_flags": provider_field(typing.Any, default = None),
        "requires_archives": provider_field(typing.Any, default = None),
        "requires_objects": provider_field(typing.Any, default = None),
        "supports_distributed_thinlto": provider_field(typing.Any, default = None),
        "independent_shlib_interface_linker_flags": provider_field(typing.Any, default = None),
        "thin_lto_premerger_enabled": provider_field(bool, default = False),
        "thin_lto_double_codegen_enabled": provider_field(bool, default = False),
        "type": LinkerType,
        "use_archiver_flags": provider_field(typing.Any, default = None),
        "force_full_hybrid_if_capable": provider_field(typing.Any, default = None),
        "is_pdb_generated": provider_field(typing.Any, default = None),  # bool
        # Flags to use to "sandbox" exported library linker flags.
        "push_pop_state_flags": provider_field(typing.Any, default = None),
        "supports_content_based_paths_for_archiving": provider_field(bool, default = False),
        "incremental_linking": provider_field(str, default = "disabled"),
        "supports_shared_libraries": provider_field(bool, default = True),
    },
)

BinaryUtilitiesInfo = provider(
    fields = {
        "bolt": provider_field(typing.Any, default = None),
        "custom_tools": provider_field(dict[str, RunInfo], default = {}),
        "dwp": provider_field(typing.Any, default = None),
        "elf_stamp": provider_field([RunInfo, None], default = None),
        "nm": provider_field(typing.Any, default = None),
        "objcopy": provider_field(typing.Any, default = None),
        "objdump": provider_field(typing.Any, default = None),
        "ranlib": provider_field(typing.Any, default = None),
        "strip": provider_field(typing.Any, default = None),
    }
)

StripFlagsInfo = provider(
    # @unsorted-dict-items
    fields = {
        "strip_debug_flags": provider_field(typing.Any, default = None),  # [["str"], None]
        "strip_non_global_flags": provider_field(typing.Any, default = None),  # [["str"], None]
        "strip_all_flags": provider_field(typing.Any, default = None),  # [["str"], None]
    },
)

DepTrackingMode = enum(
    # MAKEFILE corresponds to `gcc -MD -MF depfile ...` on *nix
    "makefile",
    # SHOW_INCLUDES corresponds to `cl.exe /showIncludes ...` on windows
    "show_includes",
    # SHOW_HEADERS corresponds to `clang/gcc -H ...` on *nix
    "show_headers",
    # Some compilers - like ml64 - do not produce information about included files
    "none",
)

# TODO(T110378147): There's a bunch of info encoded in random places in buck
# derived from information in these toolchains but hardcoded (for example,
# which file extensions are preprocessable/compilable). We should figure out
# how to move most of that into these toolchain infos.
# TODO(T110378146): The inclusion of compiler and preprocessor in here is really
# just a legacy thing that was never cleaned up. Historically, buck supported a
# mode where compilation was done in two, explicitly separate phases
# (preprocess and then compile). We don't support that today, and including
# both of these mostly just ends up with added complexity and with us
# duplicating flags in command lines.

# In cxx_library, we support a bunch of different types of files (ex. cuda),
# the toolchain for these follow this common pattern.
_compiler_fields = [
    "compiler",
    "compiler_type",
    "compiler_flags",
    # An optional argsfile that contains the preprocessor flags and the compiler flags.
    "argsfile",  # `cmd_args | None`
    # An optional argsfile that contains the preprocessor flags and the compiler flags,
    # formatted for xcode.
    "argsfile_xcode",  # `cmd_args | None`
    # An optional copy of `argsfile` filtered for C++20 module precompilation,
    # as an `argsfile_with_artifacts()` value.
    "argsfile_precompile",  # `cmd_args | None`
    "preprocessor",
    "preprocessor_type",
    "preprocessor_flags",
    # Controls cache upload for object files
    "allow_cache_upload",
    "supports_two_phase_compilation",
    "compiler_with_wrapper",
    "supports_content_based_paths",
]

AsCompilerInfo = provider(fields = _compiler_fields)
AsmCompilerInfo = provider(fields = _compiler_fields)
CCompilerInfo = provider(fields = _compiler_fields)
CudaCompilerInfo = provider(fields = _compiler_fields + ["compiler_for_dryrun"])
CvtresCompilerInfo = provider(fields = _compiler_fields)
CxxCompilerInfo = provider(fields = _compiler_fields)
HipCompilerInfo = provider(fields = _compiler_fields)
ObjcCompilerInfo = provider(fields = _compiler_fields)
ObjcxxCompilerInfo = provider(fields = _compiler_fields)
RcCompilerInfo = provider(fields = _compiler_fields)

def compiler_info_with_argsfiles(
    compiler_info: typing.Any, ctor: typing.Callable, argsfile: cmd_args, argsfile_xcode: cmd_args, argsfile_precompile: cmd_args | None = None
) -> typing.Any:
    fields = {k: getattr(compiler_info, k) for k in _compiler_fields}
    fields["argsfile"] = argsfile
    fields["argsfile_xcode"] = argsfile_xcode
    fields["argsfile_precompile"] = argsfile_precompile

    # CudaCompilerInfo carries fields beyond the shared set.
    compiler_for_dryrun = getattr(compiler_info, "compiler_for_dryrun", None)
    if compiler_for_dryrun != None:
        fields["compiler_for_dryrun"] = compiler_for_dryrun
    return ctor(**fields)

DistLtoToolsInfo = provider(
    fields = dict(
        planner = dict[LinkerType, RunInfo],
        opt = dict[LinkerType, RunInfo],
        prepare = dict[LinkerType, RunInfo],
        copy = RunInfo,
        archive_mapper = RunInfo,
        compiler_stats_merger = RunInfo,
    )
)

CxxInternalTools = provider(
    fields = dict(
        check_nonempty_output = RunInfo,
        clang_tidy_wrapper = RunInfo,
        concatenate_diagnostics = RunInfo,
        dep_file_processor = RunInfo,
        dist_lto = DistLtoToolsInfo,
        filter_argsfile = RunInfo,
        hmap_wrapper = RunInfo,
        make_comp_db = RunInfo,
        remap_cwd = RunInfo,
        serialized_diagnostics_to_json_wrapper = RunInfo,
        shlib_interface_link_wrapper = RunInfo,
        stderr_to_file = RunInfo,
        stub_header_unit = RunInfo,
    )
)

CxxObjectFormat = enum(
    "native",
    "bitcode",
    "embedded-bitcode",
    "swift",
)

# - For targets being built for x86_64, arm64, the fPIC flag isn't respected. Everything is fPIC.
# - For targets being built for Windows, nothing is fPIC. The flag is ignored.
# - There are many platforms (linux, etc.) where the fPIC flag is supported.
#
# As a result, we can end-up in a place where you pic + non-pic artifacts are requested
# but the platform will produce the exact same output (despite the different files).
#
# The PicBehavior is applied to avoid using link or output styles that are unsupported by a toolchain.
PicBehavior = enum(
    # Regardless of whether -fPIC is specified explicitly
    # every compiled artifact will have a position-independent representation.
    # This should be the default when targeting x86_64 + arm64.
    "always_enabled",
    # The -fPIC flag is known and changes the compiled artifact.
    "supported",
    # The -fPIC flag is unknown to this platform.
    "not_supported",
)

# Additional behavior for how to handle runtime dependencies
RuntimeDependencyHandling = enum(
    # Do no additional handling
    "no_symlink",
    # Always include runtime dependencies in a symlink tree, regardless
    # of whether shared linkage is used or not. Only include first level deps.
    "symlink_single_level_only",
    # Always include runtime dependencies in a symlink tree, regardless
    # of whether shared linkage is used or not. Include transitive deps.
    "symlink",
)

# TODO(T110378094): We should consider if we can change this from a hardcoded
# list of compiler_info to something more general. We could maybe do a list of
# compiler_info where each one also declares what extensions it supports.
# TODO(T110378145): Could we split up this Info so that each of the compilers
# could be provided by different dependencies? That would allow a target to
# only depend on the compilers it actually needs.
CxxToolchainInfo = provider(
    fields = {
        # Upload remote dep-file cache entries for compile actions that declare
        # dep files (and look them up on a regular action-cache miss).
        "allow_dep_file_cache_upload": provider_field(bool, default = False),
        "as_compiler_info": provider_field(typing.Any, default = None),
        "asm_compiler_info": provider_field(typing.Any, default = None),
        "binary_utilities_info": provider_field(typing.Any, default = None),
        "bolt_enabled": provider_field(typing.Any, default = None),
        "c_compiler_info": provider_field(typing.Any, default = None),
        # Maps cell names to their repo-relative path prefix. Used by coverage
        # prefix maps to translate cell-relative paths to repo-relative paths.
        # e.g. {"fbcode": "fbcode"} means cell "fbcode" maps to the "fbcode/"
        # subdirectory. Cells not in the map are assumed to be the repo root.
        "cell_to_path_prefix_map": provider_field(dict[str, str], default = {}),
        "clang_llvm_statistics": provider_field(typing.Any, default = None),
        "clang_remarks": provider_field(typing.Any, default = None),
        # Produce a time profiler JSON report by calling clang with `-ftime-trace`.
        "clang_trace": provider_field(bool, default = False),
        "compiler_flavor_flags": provider_field(typing.Any, default = {}),
        "cpp_dep_tracking_mode": provider_field(typing.Any, default = None),
        "cuda_compiler_info": provider_field(typing.Any, default = None),
        "cuda_dep_tracking_mode": provider_field(typing.Any, default = None),
        "cvtres_compiler_info": provider_field(typing.Any, default = None),
        "cxx_compiler_info": provider_field(typing.Any, default = None),
        "cxx_error_handler": provider_field(typing.Any, default = None),
        "default_deps": provider_field(list[Dependency], default = []),
        "dumpbin_toolchain_path": provider_field(typing.Any, default = None),
        "gcno_files": provider_field(typing.Any, default = None),
        "header_mode": provider_field(typing.Any, default = None),
        "headers_as_raw_headers_mode": provider_field(typing.Any, default = None),
        "hip_compiler_info": provider_field(typing.Any, default = None),
        # See prelude/cxx/hip_debug_extract.bzl. `hip_debug_extract` is the
        # wrapper RunInfo (None on unsupported ROCm).
        "hip_debug_extract": provider_field([RunInfo, None], default = None),
        "hip_device_debug_extract": provider_field(bool, default = False),
        "hip_gpu_archs": provider_field(list[str], default = []),
        "internal_tools": provider_field(CxxInternalTools),
        "libclang": provider_field(typing.Any, default = None),
        "linker_info": provider_field(typing.Any, default = None),
        "lipo": provider_field([RunInfo, None], default = None),
        "llvm_cgdata": provider_field([RunInfo, None], default = None),
        "llvm_link": provider_field(typing.Any, default = None),
        # Whether a top-level binary's unpacked external debug info (e.g.
        # split-dwarf .dwo files) is materialized as part of building it, via
        # DefaultInfo.other_outputs. When False, it can still be materialized
        # explicitly via the `[debuginfo]` sub-target.
        "materialize_external_debug_info": provider_field(bool, default = True),
        "minimum_os_version": provider_field([str, None], default = None),
        "objc_compiler_info": provider_field([ObjcCompilerInfo, None], default = None),
        "objcxx_compiler_info": provider_field([ObjcxxCompilerInfo, None], default = None),
        "object_format": provider_field(typing.Any, default = None),
        "pass_plugin": provider_field([Artifact, None], default = None),
        "pic_behavior": provider_field(typing.Any, default = None),
        "raw_headers_as_headers_mode": provider_field(typing.Any, default = None),
        "rc_compiler_info": provider_field(typing.Any, default = None),
        "remap_cwd": provider_field(bool, default = False),
        "runtime_dependency_handling": provider_field(RuntimeDependencyHandling),
        "split_debug_mode": provider_field(typing.Any, default = None),
        "strip_flags_info": provider_field(typing.Any, default = None),
        "supported_compile_flavors": provider_field(typing.Any, default = []),
        "target_stats_tools": provider_field([TargetStatsToolsInfo, None], default = None),
        "use_dep_files": provider_field(typing.Any, default = None),
        "use_distributed_thinlto": provider_field(typing.Any, default = None),
    },
)

# Stores "platform"/flavor name used to resolve *platform_* arguments
CxxPlatformInfo = provider(
    # @unsorted-dict-items
    fields = {
        "name": provider_field(typing.Any, default = None),
        # List of aliases used to resolve platform_deps
        "deps_aliases": provider_field(typing.Any, default = None),
    },
)

def _validate_linker_info(info: LinkerInfo):
    if info.requires_archives and info.requires_objects:
        fail("only one of `requires_archives` and `requires_objects` can be enabled")

def is_bitcode_format(format: CxxObjectFormat) -> bool:
    return format in [CxxObjectFormat("bitcode"), CxxObjectFormat("embedded-bitcode")]

def cxx_toolchain_infos(
    platform_name,
    c_compiler_info,
    cxx_compiler_info,
    linker_info,
    binary_utilities_info,
    header_mode,
    internal_tools: CxxInternalTools,
    headers_as_raw_headers_mode = None,
    raw_headers_as_headers_mode = None,
    asm_compiler_info = None,
    as_compiler_info = None,
    hip_compiler_info = None,
    hip_debug_extract = None,
    hip_device_debug_extract = False,
    hip_gpu_archs = [],
    cuda_compiler_info = None,
    cvtres_compiler_info = None,
    rc_compiler_info = None,
    object_format = CxxObjectFormat("native"),
    use_distributed_thinlto = False,
    use_dep_files = False,
    allow_dep_file_cache_upload = False,
    clang_remarks = None,
    clang_llvm_statistics = False,
    gcno_files = None,
    clang_trace = False,
    cpp_dep_tracking_mode = DepTrackingMode("none"),
    cuda_dep_tracking_mode = DepTrackingMode("none"),
    strip_flags_info = None,
    split_debug_mode = SplitDebugMode("none"),
    materialize_external_debug_info = True,
    bolt_enabled = False,
    cell_to_path_prefix_map = {},
    llvm_cgdata = None,
    llvm_link = None,
    platform_deps_aliases = [],
    pic_behavior = PicBehavior("supported"),
    dumpbin_toolchain_path = None,
    minimum_os_version = None,
    libclang = None,
    lipo = None,
    remap_cwd = False,
    compiler_flavor_flags = {},
    supported_compile_flavors = ["pic"],
    objc_compiler_info = None,
    objcxx_compiler_info = None,
    cxx_error_handler = None,
    pass_plugin = None,
    default_deps = [],
    runtime_dependency_handling = RuntimeDependencyHandling("no_symlink"),
    target_stats_tools = None,
):
    """
    Creates the collection of cxx-toolchain Infos for a cxx toolchain.

    c and c++ compiler infos are required, as is a linker info. The rest are
    optional, and it will be an error if any cxx_library or other rules have srcs
    of those other types.
    """

    # TODO(T110378099): verify types of the inner info objects.
    _validate_linker_info(linker_info)

    # Maintain backwards compatibility with ObjC compilation using the C compiler.
    if objc_compiler_info == None:
        objc_compiler_info = ObjcCompilerInfo(**{k: getattr(c_compiler_info, k, None) for k in _compiler_fields})
    if objcxx_compiler_info == None:
        objcxx_compiler_info = ObjcxxCompilerInfo(**{k: getattr(cxx_compiler_info, k, None) for k in _compiler_fields})

    toolchain_info = CxxToolchainInfo(
        as_compiler_info = as_compiler_info,
        asm_compiler_info = asm_compiler_info,
        binary_utilities_info = binary_utilities_info,
        bolt_enabled = bolt_enabled,
        cell_to_path_prefix_map = cell_to_path_prefix_map,
        c_compiler_info = c_compiler_info,
        clang_remarks = clang_remarks,
        clang_llvm_statistics = clang_llvm_statistics,
        clang_trace = clang_trace,
        cpp_dep_tracking_mode = cpp_dep_tracking_mode,
        cuda_compiler_info = cuda_compiler_info,
        cuda_dep_tracking_mode = cuda_dep_tracking_mode,
        cvtres_compiler_info = cvtres_compiler_info,
        cxx_compiler_info = cxx_compiler_info,
        dumpbin_toolchain_path = dumpbin_toolchain_path,
        gcno_files = gcno_files,
        header_mode = header_mode,
        headers_as_raw_headers_mode = headers_as_raw_headers_mode,
        hip_compiler_info = hip_compiler_info,
        hip_debug_extract = hip_debug_extract,
        hip_device_debug_extract = hip_device_debug_extract,
        hip_gpu_archs = hip_gpu_archs,
        internal_tools = internal_tools,
        libclang = libclang,
        linker_info = linker_info,
        lipo = lipo,
        llvm_cgdata = llvm_cgdata,
        llvm_link = llvm_link,
        materialize_external_debug_info = materialize_external_debug_info,
        objc_compiler_info = objc_compiler_info,
        objcxx_compiler_info = objcxx_compiler_info,
        object_format = object_format,
        pass_plugin = pass_plugin,
        compiler_flavor_flags = compiler_flavor_flags,
        pic_behavior = pic_behavior,
        raw_headers_as_headers_mode = raw_headers_as_headers_mode,
        rc_compiler_info = rc_compiler_info,
        remap_cwd = remap_cwd,
        runtime_dependency_handling = runtime_dependency_handling,
        split_debug_mode = split_debug_mode,
        strip_flags_info = strip_flags_info,
        minimum_os_version = minimum_os_version,
        use_dep_files = use_dep_files,
        allow_dep_file_cache_upload = allow_dep_file_cache_upload,
        use_distributed_thinlto = use_distributed_thinlto,
        cxx_error_handler = cxx_error_handler,
        supported_compile_flavors = supported_compile_flavors,
        default_deps = default_deps,
        target_stats_tools = target_stats_tools,
    )

    ldflags_shared_extra, ldflags_static_extra = None, None
    if linker_info.type == LinkerType("darwin") and is_valid_apple_platform_name(platform_name):
        # These flags are used in `cxx_genrule()`, using the toolchain's target sdk version is
        # the best we can do, as there's no target sdk on the rule itself.
        #
        # Without target triple, the linker will use the host OS as the target
        # which is almost always incorrect.
        apple_target_triple = apple_format_target_triple(platform_name, minimum_os_version or "")
        ldflags_shared_extra = apple_extra_darwin_linker_flags(apple_target_triple)
        ldflags_static_extra = apple_target_triple_flags(apple_target_triple)

    # Provide placeholder mappings, used primarily by cxx_genrule.
    # We don't support these buck1 placeholders since we can't take an argument.
    # $(ldflags-pic-filter <pattern>)
    # $(ldflags-shared-filter <pattern>)
    # $(ldflags-static-filter <pattern>)
    unkeyed_variables = {
        "ar": linker_info.archiver,
        "cc": c_compiler_info.compiler,
        "cflags": _shell_quote(c_compiler_info.compiler_flags),
        "cppflags": _shell_quote(c_compiler_info.preprocessor_flags),
        "cxx": cxx_compiler_info.compiler,
        "cxxflags": _shell_quote(cxx_compiler_info.compiler_flags),
        "cxxppflags": _shell_quote(cxx_compiler_info.preprocessor_flags),
        "ld": linker_info.linker,
        # NOTE(agallagher): The arg-less variants of the ldflags macro are
        # identical, and are just separate to match v1's behavior (ideally,
        # we just have a single `ldflags` macro for this case).
        "ldflags-shared": _shell_quote(linker_info.linker_flags or [], ldflags_shared_extra),
        "ldflags-static": _shell_quote(linker_info.linker_flags or [], ldflags_static_extra),
        "ldflags-static-pic": _shell_quote(linker_info.linker_flags or [], ldflags_static_extra),
        "objcopy": binary_utilities_info.objcopy,
    }

    if as_compiler_info != None:
        unkeyed_variables["as"] = as_compiler_info.compiler
        unkeyed_variables["asflags"] = _shell_quote(as_compiler_info.compiler_flags)
        unkeyed_variables["asppflags"] = _shell_quote(as_compiler_info.preprocessor_flags)

    if cuda_compiler_info != None:
        unkeyed_variables["cuda"] = cuda_compiler_info.compiler
        unkeyed_variables["cudaflags"] = _shell_quote(cuda_compiler_info.compiler_flags)
    placeholders_info = TemplatePlaceholderInfo(unkeyed_variables = unkeyed_variables)
    return [toolchain_info, placeholders_info, CxxPlatformInfo(name = platform_name, deps_aliases = platform_deps_aliases)]

def _shell_quote(xs, extra = None):
    return cmd_args(xs, extra or [], quote = "shell")

# export these things under a single "cxx" struct
cxx = struct(
    LinkerType = LinkerType,
    LinkerInfo = LinkerInfo,
    BinaryUtilitiesInfo = BinaryUtilitiesInfo,
    HipCompilerInfo = HipCompilerInfo,
    CudaCompilerInfo = CudaCompilerInfo,
    CCompilerInfo = CCompilerInfo,
    CxxCompilerInfo = CxxCompilerInfo,
    AsmCompilerInfo = AsmCompilerInfo,
    AsCompilerInfo = AsCompilerInfo,
    CxxToolchainInfo = CxxToolchainInfo,
    CxxPlatformInfo = CxxPlatformInfo,
    StripFlagsInfo = StripFlagsInfo,
    DistLtoToolsInfo = DistLtoToolsInfo,
    cxx_toolchain_infos = cxx_toolchain_infos,
)
