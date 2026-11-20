# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:is_full_meta_repo.bzl", "is_full_meta_repo")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//cxx:attrs.bzl", "cxx_common")
load("@prelude//cxx:compile.bzl", "compiler_info_with_toolchain_argsfiles")
load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "AsCompilerInfo",
    "AsmCompilerInfo",
    "BinaryUtilitiesInfo",
    "CCompilerInfo",
    "CXX_COMPILER_TYPES",
    "CudaCompilerInfo",
    "CvtresCompilerInfo",
    "CxxCompilerInfo",
    "CxxInternalTools",
    "CxxObjectFormat",
    "DepTrackingMode",
    "HipCompilerInfo",
    "IncrementalLinkingMode",
    "LinkerInfo",
    "LinkerType",
    "ObjcCompilerInfo",
    "ObjcxxCompilerInfo",
    "PicBehavior",
    "RcCompilerInfo",
    "ShlibInterfacesMode",
    "StripFlagsInfo",
    "cxx_toolchain_infos",
)
load("@prelude//cxx:cxx_utility.bzl", "cxx_toolchain_allow_cache_upload_args")
load("@prelude//cxx:debug.bzl", "SplitDebugMode")
load("@prelude//cxx:headers.bzl", "HeaderMode", "HeadersAsRawHeadersMode", "RawHeadersAsHeadersMode")
load("@prelude//cxx:linker.bzl", "LINKERS", "is_pdb_generated")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//linking:link_info.bzl", "ArchiveContentsType", "LibOutputStyle", "LinkOrdering", "LinkStyle")
load("@prelude//linking:linkable_graph.bzl", "LinkableGraph")
load("@prelude//linking:lto.bzl", "LtoMode", "lto_compiler_flags")
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
)
load("@prelude//target_stats:target_stats_tools.bzl", "TargetStatsToolsInfo")
load("@prelude//utils:utils.bzl", "flatten", "value_or")

def cxx_toolchain_impl(ctx):
    c_compiler = _get_maybe_wrapped_msvc(
        ctx.attrs.c_compiler[RunInfo], ctx.attrs.c_compiler_type or ctx.attrs.compiler_type, ctx.attrs._msvc_hermetic_exec[RunInfo]
    )

    lto_mode = LtoMode(ctx.attrs.lto_mode)
    if lto_mode != LtoMode("none"):
        object_format = "bitcode"
    else:
        object_format = ctx.attrs.object_format if ctx.attrs.object_format else "native"

    c_lto_flags = lto_compiler_flags(lto_mode)

    c_info = CCompilerInfo(
        compiler = c_compiler,
        compiler_type = ctx.attrs.c_compiler_type or ctx.attrs.compiler_type,
        compiler_flags = cmd_args(ctx.attrs.c_compiler_flags, c_lto_flags),
        preprocessor = c_compiler,
        preprocessor_flags = cmd_args(ctx.attrs.c_preprocessor_flags),
        allow_cache_upload = ctx.attrs.c_compiler_allow_cache_upload,
        supports_content_based_paths = ctx.attrs.supports_content_based_paths,
    )
    objc_info = ObjcCompilerInfo(
        compiler = c_compiler,
        compiler_type = ctx.attrs.c_compiler_type or ctx.attrs.compiler_type,
        compiler_flags = cmd_args(ctx.attrs.c_compiler_flags, c_lto_flags, ctx.attrs.objc_compiler_flags),
        preprocessor = c_compiler,
        preprocessor_flags = cmd_args(ctx.attrs.c_preprocessor_flags),
        allow_cache_upload = ctx.attrs.c_compiler_allow_cache_upload,
        supports_content_based_paths = ctx.attrs.supports_content_based_paths,
    )
    cxx_compiler = _get_maybe_wrapped_msvc(
        ctx.attrs.cxx_compiler[RunInfo], ctx.attrs.cxx_compiler_type or ctx.attrs.compiler_type, ctx.attrs._msvc_hermetic_exec[RunInfo]
    )
    cxx_info = CxxCompilerInfo(
        compiler = cxx_compiler,
        compiler_type = ctx.attrs.cxx_compiler_type or ctx.attrs.compiler_type,
        compiler_flags = cmd_args(ctx.attrs.cxx_compiler_flags, c_lto_flags),
        preprocessor = cxx_compiler,
        preprocessor_flags = cmd_args(ctx.attrs.cxx_preprocessor_flags),
        allow_cache_upload = ctx.attrs.cxx_compiler_allow_cache_upload,
        supports_two_phase_compilation = ctx.attrs.supports_two_phase_compilation,
        supports_content_based_paths = ctx.attrs.supports_content_based_paths,
    )
    objcxx_info = ObjcxxCompilerInfo(
        compiler = cxx_compiler,
        compiler_type = ctx.attrs.cxx_compiler_type or ctx.attrs.compiler_type,
        compiler_flags = cmd_args(ctx.attrs.cxx_compiler_flags, c_lto_flags, ctx.attrs.objcxx_compiler_flags),
        preprocessor = cxx_compiler,
        preprocessor_flags = cmd_args(ctx.attrs.cxx_preprocessor_flags),
        allow_cache_upload = ctx.attrs.cxx_compiler_allow_cache_upload,
        supports_content_based_paths = ctx.attrs.supports_content_based_paths,
    )
    asm_info = (
        AsmCompilerInfo(
            compiler = ctx.attrs.asm_compiler[RunInfo],
            compiler_type = ctx.attrs.asm_compiler_type or ctx.attrs.compiler_type,
            compiler_flags = cmd_args(ctx.attrs.asm_compiler_flags),
            preprocessor_flags = cmd_args(ctx.attrs.asm_preprocessor_flags),
            supports_content_based_paths = ctx.attrs.supports_content_based_paths,
        )
        if ctx.attrs.asm_compiler
        else None
    )
    as_info = (
        AsCompilerInfo(
            compiler = ctx.attrs.assembler[RunInfo],
            compiler_type = ctx.attrs.assembler_type or ctx.attrs.compiler_type,
            compiler_flags = cmd_args(ctx.attrs.assembler_flags),
            preprocessor_flags = cmd_args(ctx.attrs.assembler_preprocessor_flags),
            supports_content_based_paths = ctx.attrs.supports_content_based_paths,
        )
        if ctx.attrs.assembler
        else None
    )
    cuda_info = (
        CudaCompilerInfo(
            compiler = ctx.attrs.cuda_compiler[RunInfo],
            compiler_for_dryrun = ctx.attrs.cuda_compiler_for_dryrun[RunInfo] if ctx.attrs.cuda_compiler_for_dryrun else None,
            compiler_type = ctx.attrs.cuda_compiler_type or ctx.attrs.compiler_type,
            compiler_flags = cmd_args(ctx.attrs.cuda_compiler_flags),
            preprocessor_flags = cmd_args(ctx.attrs.cuda_preprocessor_flags),
            allow_cache_upload = ctx.attrs.cuda_compiler_allow_cache_upload,
            supports_content_based_paths = ctx.attrs.supports_content_based_paths,
        )
        if ctx.attrs.cuda_compiler
        else None
    )
    hip_info = (
        HipCompilerInfo(
            compiler = ctx.attrs.hip_compiler[RunInfo],
            compiler_type = ctx.attrs.hip_compiler_type or ctx.attrs.compiler_type,
            compiler_flags = cmd_args(ctx.attrs.hip_compiler_flags),
            preprocessor_flags = cmd_args(ctx.attrs.hip_preprocessor_flags),
        )
        if ctx.attrs.hip_compiler
        else None
    )
    cvtres_info = (
        CvtresCompilerInfo(
            compiler = ctx.attrs.cvtres_compiler[RunInfo],
            compiler_type = ctx.attrs.cvtres_compiler_type or ctx.attrs.compiler_type,
            compiler_flags = cmd_args(ctx.attrs.cvtres_compiler_flags),
            preprocessor_flags = cmd_args(ctx.attrs.cvtres_preprocessor_flags),
        )
        if ctx.attrs.cvtres_compiler
        else None
    )
    rc_info = (
        RcCompilerInfo(
            compiler = ctx.attrs.rc_compiler[RunInfo],
            compiler_type = ctx.attrs.rc_compiler_type or ctx.attrs.compiler_type,
            compiler_flags = cmd_args(ctx.attrs.rc_compiler_flags),
            preprocessor_flags = cmd_args(ctx.attrs.rc_preprocessor_flags),
        )
        if ctx.attrs.rc_compiler
        else None
    )

    c_info = compiler_info_with_toolchain_argsfiles(ctx.actions, "c", CCompilerInfo, c_info)
    objc_info = compiler_info_with_toolchain_argsfiles(ctx.actions, "objc", ObjcCompilerInfo, objc_info)
    cxx_info = compiler_info_with_toolchain_argsfiles(
        ctx.actions,
        "cxx",
        CxxCompilerInfo,
        cxx_info,
        # Only the cxx info can be precompiled for C++20 modules.
        precompile_filter = ctx.attrs.internal_tools[CxxInternalTools].filter_argsfile,
    )
    objcxx_info = compiler_info_with_toolchain_argsfiles(ctx.actions, "objcxx", ObjcxxCompilerInfo, objcxx_info)
    asm_info = compiler_info_with_toolchain_argsfiles(ctx.actions, "asm", AsmCompilerInfo, asm_info)
    as_info = compiler_info_with_toolchain_argsfiles(ctx.actions, "as", AsCompilerInfo, as_info)
    cuda_info = compiler_info_with_toolchain_argsfiles(ctx.actions, "cuda", CudaCompilerInfo, cuda_info)
    hip_info = compiler_info_with_toolchain_argsfiles(ctx.actions, "hip", HipCompilerInfo, hip_info)
    cvtres_info = compiler_info_with_toolchain_argsfiles(ctx.actions, "cvtres", CvtresCompilerInfo, cvtres_info)
    rc_info = compiler_info_with_toolchain_argsfiles(ctx.actions, "rc", RcCompilerInfo, rc_info)

    linker_type = LinkerType(ctx.attrs.linker_type)
    linker_info = LinkerInfo(
        archiver = ctx.attrs.archiver[RunInfo],
        archiver_flags = cmd_args(ctx.attrs.archiver_flags),
        archiver_reads_inputs = ctx.attrs.archiver_reads_inputs,
        archiver_supports_argfiles = ctx.attrs.archiver_supports_argfiles,
        archiver_type = ctx.attrs.archiver_type,
        archive_contents = ctx.attrs.archive_contents,
        archive_objects_locally = False,
        archive_symbol_table = ctx.attrs.archive_symbol_table,
        binary_extension = value_or(ctx.attrs.binary_extension, ""),
        extra_outputs = ctx.attrs.extra_linker_outputs,
        generate_linker_maps = ctx.attrs.generate_linker_maps,
        generate_gc_sections = ctx.attrs.generate_gc_sections,
        is_pdb_generated = is_pdb_generated(linker_type, ctx.attrs.linker_flags),
        link_binaries_locally = not value_or(ctx.attrs.cache_links, True),
        link_libraries_locally = False,
        link_style = LinkStyle(ctx.attrs.link_style),
        link_weight = ctx.attrs.link_weight,
        link_ordering = ctx.attrs.link_ordering,
        linker = ctx.attrs.linker[RunInfo],
        linker_flags = cmd_args(ctx.attrs.linker_flags, c_lto_flags),
        executable_linker_flags = ctx.attrs.executable_linker_flags,
        binary_linker_flags = ctx.attrs.binary_linker_flags,
        dist_thin_lto_codegen_flags = cmd_args(ctx.attrs.dist_thin_lto_codegen_flags) if ctx.attrs.dist_thin_lto_codegen_flags else cmd_args(),
        post_linker_flags = cmd_args(ctx.attrs.post_linker_flags),
        link_metadata_flag = ctx.attrs.link_metadata_flag,
        lto_mode = lto_mode,
        mk_shlib_intf = ctx.attrs.shared_library_interface_producer,
        object_file_extension = ctx.attrs.object_file_extension or "o",
        shlib_interfaces = ShlibInterfacesMode(ctx.attrs.shared_library_interface_mode),
        independent_shlib_interface_linker_flags = ctx.attrs.shared_library_interface_flags,
        requires_archives = value_or(ctx.attrs.requires_archives, True),
        requires_objects = value_or(ctx.attrs.requires_objects, False),
        sanitizer_runtime_enabled = ctx.attrs.sanitizer_runtime_enabled,
        sanitizer_runtime_files = flatten([runtime_file[DefaultInfo].default_outputs for runtime_file in ctx.attrs.sanitizer_runtime_files]),
        runtime_library_files = [
            dep[LinkableGraph].nodes.value.linkable.link_infos[LibOutputStyle("pic_archive")].default for dep in ctx.attrs.runtime_library_files
        ],
        supports_distributed_thinlto = ctx.attrs.supports_distributed_thinlto,
        shared_dep_runtime_ld_flags = ctx.attrs.shared_dep_runtime_ld_flags,
        shared_library_name_default_prefix = _get_shared_library_name_default_prefix(ctx),
        shared_library_name_format = _get_shared_library_name_format(ctx),
        shared_library_versioned_name_format = _get_shared_library_versioned_name_format(ctx),
        static_dep_runtime_ld_flags = ctx.attrs.static_dep_runtime_ld_flags,
        static_library_extension = ctx.attrs.static_library_extension or "a",
        static_pic_dep_runtime_ld_flags = ctx.attrs.static_pic_dep_runtime_ld_flags,
        thin_lto_double_codegen_enabled = ctx.attrs.thin_lto_double_codegen_enabled,
        type = linker_type,
        use_archiver_flags = ctx.attrs.use_archiver_flags,
        incremental_linking = ctx.attrs.incremental_linking,
        supports_content_based_paths_for_archiving = ctx.attrs.supports_content_based_paths_for_archiving,
        supports_shared_libraries = ctx.attrs.supports_shared_libraries,
    )

    utilities_info = BinaryUtilitiesInfo(
        bolt = ctx.attrs.bolt[RunInfo] if ctx.attrs.bolt else None,
        custom_tools = {name: dep[RunInfo] for name, dep in ctx.attrs.custom_tools.items()},
        elf_stamp = ctx.attrs.elf_stamp[RunInfo] if ctx.attrs.elf_stamp else None,
        nm = ctx.attrs.nm[RunInfo],
        objcopy = ctx.attrs.objcopy_for_shared_library_interface[RunInfo],
        objdump = ctx.attrs.objdump[RunInfo] if ctx.attrs.objdump else None,
        ranlib = ctx.attrs.ranlib[RunInfo] if ctx.attrs.ranlib else None,
        strip = ctx.attrs.strip[RunInfo],
        dwp = ctx.attrs.dwp[RunInfo] if ctx.attrs.dwp else None,
    )

    strip_flags_info = StripFlagsInfo(
        strip_debug_flags = ctx.attrs.strip_debug_flags,
        strip_non_global_flags = ctx.attrs.strip_non_global_flags,
        strip_all_flags = ctx.attrs.strip_all_flags,
    )

    platform_name = ctx.attrs.platform_name or ctx.attrs.name
    return [
        DefaultInfo(),
    ] + cxx_toolchain_infos(
        as_compiler_info = as_info,
        asm_compiler_info = asm_info,
        binary_utilities_info = utilities_info,
        bolt_enabled = value_or(ctx.attrs.bolt_enabled, False),
        cell_to_path_prefix_map = ctx.attrs.cell_to_path_prefix_map,
        c_compiler_info = c_info,
        clang_remarks = ctx.attrs.clang_remarks,
        clang_llvm_statistics = value_or(ctx.attrs.clang_llvm_statistics, False),
        clang_trace = value_or(ctx.attrs.clang_trace, False),
        cpp_dep_tracking_mode = DepTrackingMode(ctx.attrs.cpp_dep_tracking_mode),
        cuda_compiler_info = cuda_info,
        cuda_dep_tracking_mode = DepTrackingMode(ctx.attrs.cuda_dep_tracking_mode),
        cvtres_compiler_info = cvtres_info,
        cxx_compiler_info = cxx_info,
        dumpbin_toolchain_path = ctx.attrs._dumpbin_toolchain_path[DefaultInfo].default_outputs[0] if ctx.attrs._dumpbin_toolchain_path else None,
        gcno_files = value_or(ctx.attrs.gcno_files, False),
        header_mode = _get_header_mode(ctx),
        headers_as_raw_headers_mode = HeadersAsRawHeadersMode(ctx.attrs.headers_as_raw_headers_mode) if ctx.attrs.headers_as_raw_headers_mode != None else None,
        hip_compiler_info = hip_info,
        internal_tools = ctx.attrs.internal_tools[CxxInternalTools],
        libclang = ctx.attrs.libclang,
        linker_info = linker_info,
        lipo = ctx.attrs.lipo[RunInfo] if ctx.attrs.lipo else None,
        llvm_cgdata = ctx.attrs.llvm_cgdata[RunInfo] if ctx.attrs.llvm_cgdata else None,
        llvm_link = ctx.attrs.llvm_link[RunInfo] if ctx.attrs.llvm_link else None,
        objc_compiler_info = objc_info,
        objcxx_compiler_info = objcxx_info,
        object_format = CxxObjectFormat(object_format),
        pass_plugin = ctx.attrs.pass_plugin[DefaultInfo].default_outputs[0] if ctx.attrs.pass_plugin else None,
        compiler_flavor_flags = ctx.attrs.compiler_flavor_flags,
        compiler_flavor_lang_compiler_flags = ctx.attrs.compiler_flavor_lang_compiler_flags,
        pic_behavior = PicBehavior(ctx.attrs.pic_behavior),
        platform_deps_aliases = ctx.attrs.platform_deps_aliases,
        platform_name = platform_name,
        raw_headers_as_headers_mode = RawHeadersAsHeadersMode(ctx.attrs.raw_headers_as_headers_mode) if ctx.attrs.raw_headers_as_headers_mode != None else None,
        rc_compiler_info = rc_info,
        remap_cwd = ctx.attrs.remap_cwd,
        materialize_external_debug_info = ctx.attrs.materialize_external_debug_info,
        split_debug_mode = SplitDebugMode(ctx.attrs.split_debug_mode),
        strip_flags_info = strip_flags_info,
        minimum_os_version = ctx.attrs.minimum_os_version,
        # TODO(T138705365): Turn on dep files by default
        use_dep_files = value_or(ctx.attrs.use_dep_files, _get_default_use_dep_files(platform_name)),
        allow_dep_file_cache_upload = ctx.attrs.allow_dep_file_cache_upload,
        default_deps = ctx.attrs.default_deps,
        target_stats_tools = ctx.attrs.target_stats_tools[TargetStatsToolsInfo] if ctx.attrs.target_stats_tools else None,
    )

def cxx_toolchain_extra_attributes(is_toolchain_rule):
    dep_type = attrs.exec_dep if is_toolchain_rule else attrs.dep
    return {
        "allow_dep_file_cache_upload": attrs.bool(default = False),
        "archive_symbol_table": attrs.bool(default = True),
        "archiver": dep_type(providers = [RunInfo]),
        "archiver_reads_inputs": attrs.bool(default = True),
        "archiver_supports_argfiles": attrs.bool(default = False),
        "asm_compiler": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "asm_preprocessor": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "assembler": dep_type(providers = [RunInfo]),
        "assembler_preprocessor": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "bolt": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "bolt_enabled": attrs.bool(default = False),
        "c_compiler": dep_type(providers = [RunInfo]),
        "cell_to_path_prefix_map": attrs.dict(key = attrs.string(), value = attrs.string(), default = {}),
        "clang_llvm_statistics": attrs.option(attrs.bool(), default = None),
        "clang_remarks": attrs.option(attrs.string(), default = None),
        "clang_trace": attrs.option(attrs.bool(), default = None),
        "compiler_flavor_flags": attrs.dict(key = attrs.string(), value = attrs.list(attrs.string()), default = {}),
        "compiler_flavor_lang_compiler_flags": attrs.dict(
            key = attrs.string(), value = attrs.dict(key = attrs.string(), value = attrs.list(attrs.string())), default = {}
        ),
        "cpp_dep_tracking_mode": attrs.enum(DepTrackingMode.values(), default = "makefile"),
        "cuda_compiler": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "cuda_compiler_for_dryrun": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "cuda_dep_tracking_mode": attrs.enum(DepTrackingMode.values(), default = "makefile"),
        "custom_tools": attrs.dict(key = attrs.string(), value = dep_type(providers = [RunInfo]), default = {}),
        "cvtres_compiler": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "cxx_compiler": dep_type(providers = [RunInfo]),
        "default_deps": attrs.list(dep_type(), default = []),
        "dwp": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "elf_stamp": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "gcno_files": attrs.bool(default = False),
        "generate_gc_sections": attrs.bool(default = False),
        "generate_linker_maps": attrs.bool(default = False),
        "hip_compiler": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "incremental_linking": attrs.enum(IncrementalLinkingMode.values(), default = "disabled"),
        "internal_tools": dep_type(providers = [CxxInternalTools], default = "prelude//cxx/tools:internal_tools"),
        "libclang": attrs.option(dep_type(providers = [SharedLibraryInfo]), default = None),
        "link_ordering": attrs.enum(LinkOrdering.values(), default = "preorder"),
        "link_weight": attrs.int(default = 1),
        "linker": dep_type(providers = [RunInfo]),
        "lipo": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "llvm_cgdata": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "llvm_link": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "lto_mode": attrs.enum(LtoMode.values(), default = "none"),
        "materialize_external_debug_info": attrs.bool(default = True),
        # Darwin only: the deployment target to use for this build
        "minimum_os_version": attrs.option(attrs.string(), default = None),
        "nm": dep_type(providers = [RunInfo]),
        "objcopy_for_shared_library_interface": dep_type(providers = [RunInfo]),
        "objdump": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "object_format": attrs.enum(CxxObjectFormat.values(), default = "native"),
        "pass_plugin": attrs.option(dep_type(providers = [DefaultInfo]), default = None),
        "pic_behavior": attrs.enum(PicBehavior.values(), default = "supported"),
        # A placeholder tool that can be used to set up toolchain constraints.
        # Useful when fat and thin toolchahins share the same underlying tools via `command_alias()`,
        # which requires setting up separate platform-specific aliases with the correct constraints.
        "placeholder_tool": attrs.option(dep_type(providers = [RunInfo]), default = None),
        # Used for regex matching any 'platform_*' attributes.
        "platform_deps_aliases": attrs.option(attrs.list(attrs.string()), default = None),
        # Used for regex matching any 'platform_*' attributes.
        "platform_name": attrs.option(attrs.string(), default = None),
        "private_headers_symlinks_enabled": attrs.bool(default = True),
        "public_headers_symlinks_enabled": attrs.bool(default = True),
        "ranlib": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "rc_compiler": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "remap_cwd": attrs.bool(default = False),
        "requires_objects": attrs.bool(default = False),
        "runtime_library_files": attrs.set(attrs.dep(), sorted = True, default = []),  # Use `attrs.dep()` as it's not a tool, always propagate target platform
        "sanitizer_runtime_enabled": attrs.bool(default = False),
        "sanitizer_runtime_files": attrs.set(attrs.dep(), sorted = True, default = []),  # Use `attrs.dep()` as it's not a tool, always propagate target platform
        "shared_library_interface_mode": attrs.enum(ShlibInterfacesMode.values(), default = "disabled"),
        "shared_library_interface_producer": attrs.option(dep_type(providers = [RunInfo]), default = None),
        "split_debug_mode": attrs.enum(SplitDebugMode.values(), default = "none"),
        "strip": dep_type(providers = [RunInfo]),
        "supports_content_based_paths": attrs.bool(default = False),
        "supports_content_based_paths_for_archiving": attrs.bool(default = False),
        "supports_distributed_thinlto": attrs.bool(default = False),
        "supports_shared_libraries": attrs.bool(default = True),
        "supports_two_phase_compilation": attrs.bool(default = False),
        "target_stats_tools": attrs.option(dep_type(providers = [TargetStatsToolsInfo]), default = None),
        "thin_lto_double_codegen_enabled": attrs.bool(default = False),
        "use_archiver_flags": attrs.bool(default = True),
        "use_dep_files": attrs.option(attrs.bool(), default = None),
        # TODO(scottcao): Figure out a slightly better way to integrate this. In theory, this is only needed for clang toolchain.
        # If we were using msvc, we should be able to use dumpbin directly.
        "_dumpbin_toolchain_path": attrs.default_only(
            attrs.option(
                dep_type(providers = [DefaultInfo]),
                default = select({
                    "DEFAULT": None,
                    "ovr_config//os:windows": select({
                        # Unfortunately, it seems like an unresolved select when resolve exec platforms causes the whole resolution
                        # to fail, so I need a DEFAULT here when some target without cpu constraint tries to configure against the
                        # windows exec platform.
                        "DEFAULT": None,
                        # FIXME: prelude// should be standalone (not refer to fbsource//)
                        "ovr_config//cpu:x86_32": "fbsource//third-party/toolchains/visual_studio:cl_x86_and_tools",
                        "ovr_config//cpu:x86_64": "fbsource//third-party/toolchains/visual_studio:cl_x64_and_tools",
                    }),
                })
                if is_full_meta_repo()
                else None,
            )
        ),
        "_msvc_hermetic_exec": attrs.default_only(dep_type(providers = [RunInfo], default = "prelude//windows/tools:msvc_hermetic_exec")),
    } | cxx_toolchain_allow_cache_upload_args()

ArchiverProviderType = ["bsd", "gnu", "llvm", "windows", "windows_clang"]

LinkerProviderType = ["darwin", "gnu", "windows", "unknown", "wasm"]

PicType = ["pic", "pdc"]

def _cxx_toolchain_base_attrs() -> dict[str, Attr]:
    return (
        cxx_common.raw_headers_as_headers_mode_arg()
        | {
            "archive_contents": attrs.enum(ArchiveContentsType.values(), default = "normal"),
            "archiver": attrs.source(),
            "archiver_flags": attrs.list(attrs.arg(), default = []),
            "archiver_type": attrs.enum(ArchiverProviderType),
            "asm_compiler": attrs.option(attrs.source(), default = None),
            "asm_compiler_flags": attrs.list(attrs.arg(), default = []),
            "asm_compiler_type": attrs.option(attrs.enum(CXX_COMPILER_TYPES), default = None),
            "asm_preprocessor": attrs.option(attrs.source(), default = None),
            "asm_preprocessor_flags": attrs.list(attrs.arg(), default = []),
            "asm_preprocessor_type": attrs.option(attrs.enum(CXX_COMPILER_TYPES), default = None),
            "assembler": attrs.source(),
            "assembler_flags": attrs.list(attrs.arg(), default = []),
            "assembler_preprocessor": attrs.option(attrs.source(), default = None),
            "assembler_preprocessor_flags": attrs.list(attrs.arg(), default = []),
            "assembler_preprocessor_type": attrs.option(attrs.enum(CXX_COMPILER_TYPES), default = None),
            "assembler_type": attrs.option(attrs.enum(CXX_COMPILER_TYPES), default = None),
            "binary_extension": attrs.option(attrs.string(), default = None),
            "binary_linker_flags": attrs.list(
                attrs.arg(anon_target_compatible = True),
                default = [],
                doc = """
                Linker flags that apply to all links coordinated by a binary
                rule.  One key distinction between these and `executable_linker_flags`
                is that these will also apply to library links coordinated by
                binary rules (e.g. linking roots/deps when using native python or
                omnibus link strategies).
                """,
            ),
            "bolt": attrs.source(),
            "c_compiler": attrs.source(),
            "c_compiler_flags": attrs.list(attrs.arg(), default = []),
            "c_compiler_type": attrs.option(attrs.enum(CXX_COMPILER_TYPES), default = None),
            "c_preprocessor_flags": attrs.list(attrs.arg(), default = []),
            "cache_links": attrs.bool(default = False),
            "compiler_type": attrs.option(attrs.enum(CXX_COMPILER_TYPES), default = None),
            "cuda_compiler": attrs.option(attrs.source(), default = None),
            "cuda_compiler_flags": attrs.list(attrs.arg(), default = []),
            "cuda_compiler_type": attrs.option(attrs.enum(CXX_COMPILER_TYPES), default = None),
            "cuda_preprocessor_flags": attrs.list(attrs.arg(), default = []),
            "custom_tools": attrs.dict(key = attrs.string(), value = attrs.source(), default = {}),
            "cvtres_compiler": attrs.option(attrs.source(), default = None),
            "cvtres_compiler_flags": attrs.list(attrs.arg(), default = []),
            "cvtres_compiler_type": attrs.option(attrs.enum(CXX_COMPILER_TYPES), default = None),
            "cvtres_preprocessor_flags": attrs.list(attrs.arg(), default = []),
            "cxx_compiler": attrs.source(),
            "cxx_compiler_flags": attrs.list(attrs.arg(), default = []),
            "cxx_compiler_type": attrs.option(attrs.enum(CXX_COMPILER_TYPES), default = None),
            "cxx_preprocessor_flags": attrs.list(attrs.arg(), default = []),
            "debug_path_prefix_map_sanitizer_format": attrs.option(attrs.string(), default = None),
            "dist_thin_lto_codegen_flags": attrs.list(attrs.arg(), default = []),
            "executable_linker_flags": attrs.list(
                attrs.arg(anon_target_compatible = True),
                default = [],
                doc = """
                Linker flags that only apply when linking an executable.
                """,
            ),
            "extra_linker_outputs": attrs.set(attrs.string(), default = []),
            "headers_as_raw_headers_mode": attrs.option(attrs.enum(HeadersAsRawHeadersMode.values()), default = None),
            "hip_compiler": attrs.option(attrs.source(), default = None),
            "hip_compiler_flags": attrs.list(attrs.arg(), default = []),
            "hip_compiler_type": attrs.option(attrs.enum(CXX_COMPILER_TYPES), default = None),
            "hip_preprocessor_flags": attrs.list(attrs.arg(), default = []),
            "link_metadata_flag": attrs.option(attrs.string(), default = None),
            "link_style": attrs.enum(
                LinkStyle.values(),
                default = "static",
                doc = """
                The default value of the `link_style` attribute for rules that use this toolchain.
                """,
            ),
            "linker": attrs.source(),
            "linker_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
            "linker_type": attrs.enum(LinkerProviderType),
            "nm": attrs.source(),
            "objc_compiler_flags": attrs.list(attrs.arg(), default = []),
            "objcopy_for_shared_library_interface": attrs.source(),
            "objcxx_compiler_flags": attrs.list(attrs.arg(), default = []),
            "objdump": attrs.option(attrs.source(), default = None),
            "object_file_extension": attrs.string(default = ""),
            "post_linker_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
            "private_headers_symlinks_enabled": attrs.bool(default = False),
            "public_headers_symlinks_enabled": attrs.bool(default = False),
            "ranlib": attrs.option(attrs.source(), default = None),
            "ranlib_flags": attrs.list(attrs.arg(), default = []),
            "rc_compiler": attrs.option(attrs.source(), default = None),
            "rc_compiler_flags": attrs.list(attrs.arg(), default = []),
            "rc_compiler_type": attrs.option(attrs.enum(CXX_COMPILER_TYPES), default = None),
            "rc_preprocessor_flags": attrs.list(attrs.arg(), default = []),
            "requires_archives": attrs.bool(default = False),
            "shared_dep_runtime_ld_flags": attrs.list(attrs.arg(), default = []),
            "shared_library_extension": attrs.string(default = ""),
            "shared_library_interface_flags": attrs.list(attrs.string(), default = []),
            "shared_library_versioned_extension_format": attrs.string(default = ""),
            "static_dep_runtime_ld_flags": attrs.list(attrs.arg(), default = []),
            "static_library_extension": attrs.string(default = ""),
            "static_pic_dep_runtime_ld_flags": attrs.list(attrs.arg(), default = []),
            "strip": attrs.source(),
            "strip_all_flags": attrs.option(attrs.list(attrs.arg()), default = None),
            "strip_debug_flags": attrs.option(attrs.list(attrs.arg()), default = None),
            "strip_non_global_flags": attrs.option(attrs.list(attrs.arg()), default = None),
            "use_header_map": attrs.bool(default = False),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    )

cxx_toolchain_inheriting_target_platform = rule(
    impl = cxx_toolchain_impl,
    attrs = _cxx_toolchain_base_attrs() | cxx_toolchain_extra_attributes(is_toolchain_rule = True),
    is_toolchain_rule = True,
)

_APPLE_PLATFORM_NAME_PREFIXES = [
    "iphonesimulator",
    "iphoneos",
    "maccatalyst",
    "macosx",
    "watchos",
    "watchsimulator",
    "appletvos",
    "appletvsimulator",
]

def _get_default_use_dep_files(platform_name: str) -> bool:
    # All Apple platforms use Clang which supports the standard dep files format
    for apple_platform_name_prefix in _APPLE_PLATFORM_NAME_PREFIXES:
        if apple_platform_name_prefix in platform_name:
            return True
    return False

def _get_header_mode(ctx: AnalysisContext) -> HeaderMode:
    if ctx.attrs.use_header_map:
        if ctx.attrs.private_headers_symlinks_enabled or ctx.attrs.public_headers_symlinks_enabled:
            return HeaderMode("symlink_tree_with_header_map")
        else:
            return HeaderMode("header_map_only")
    else:
        return HeaderMode("symlink_tree_only")

def _get_shared_library_name_default_prefix(ctx: AnalysisContext) -> str:
    extension = ctx.attrs.shared_library_extension
    return "" if extension == "dll" else "lib"

def _get_shared_library_name_format(ctx: AnalysisContext) -> str:
    linker_type = LinkerType(ctx.attrs.linker_type)
    extension = ctx.attrs.shared_library_extension
    if extension == "":
        extension = LINKERS[linker_type].default_shared_library_extension
    return "{}." + extension

def _get_shared_library_versioned_name_format(ctx: AnalysisContext) -> str:
    linker_type = LinkerType(ctx.attrs.linker_type)
    extension_format = ctx.attrs.shared_library_versioned_extension_format.replace("%s", "{}")
    if extension_format == "":
        extension_format = LINKERS[linker_type].default_shared_library_versioned_extension_format
    return "{}." + extension_format

def _get_maybe_wrapped_msvc(compiler: RunInfo, compiler_type: str, msvc_hermetic_exec: RunInfo) -> RunInfo:
    if compiler_type == "windows":
        return RunInfo(args = cmd_args(msvc_hermetic_exec, compiler))
    return compiler

cxx_toolchain = prelude_rule(
    name = "cxx_toolchain",
    impl = cxx_toolchain_impl,
    docs = """
        A `cxx_toolchain()` rule declares a complete C/C++ toolchain — a
        set of compilers (C, C++, Objective-C, asm, CUDA, HIP, etc.),
        preprocessors, linker, archiver, and binary utilities (`strip`,
        `objcopy`, `dwp`, `nm`, etc.) — along with the flags and behavior
        options that control how each tool is invoked.

        Native rules (`cxx_binary()`, `cxx_library()`, `cxx_test()`,
        `prebuilt_cxx_library()`, etc.) and rules in other languages that
        ultimately link native code (e.g. `python_binary()`,
        `rust_binary()`, `haskell_binary()`) consume a `cxx_toolchain()`
        via a toolchain dependency to compile and link their outputs.
    """,
    examples = None,
    further = None,
    attrs = _cxx_toolchain_base_attrs() | cxx_toolchain_extra_attributes(is_toolchain_rule = False),
)
