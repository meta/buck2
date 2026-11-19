# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "BinaryUtilitiesInfo",
    "CCompilerInfo",
    "CvtresCompilerInfo",
    "CxxCompilerInfo",
    "CxxInternalTools",
    "CxxPlatformInfo",
    "CxxToolchainInfo",
    "DepTrackingMode",
    "LinkerInfo",
    "LinkerType",
    "PicBehavior",
    "RcCompilerInfo",
    "RuntimeDependencyHandling",
    "ShlibInterfacesMode",
)
load("@prelude//cxx:headers.bzl", "HeaderMode")
load("@prelude//cxx:linker.bzl", "is_pdb_generated")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//linking:link_info.bzl", "LinkOrdering", "LinkStyle")
load("@prelude//linking:lto.bzl", "LtoMode")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")

CxxToolsInfo = provider(
    fields = {
        "archiver": provider_field(typing.Any, default = None),
        "archiver_type": provider_field(typing.Any, default = None),
        "asm_compiler": provider_field(typing.Any, default = None),
        "asm_compiler_type": provider_field(typing.Any, default = None),
        "compiler": provider_field(typing.Any, default = None),
        "compiler_type": provider_field(typing.Any, default = None),
        "cvtres_compiler": provider_field(typing.Any, default = None),
        "cxx_compiler": provider_field(typing.Any, default = None),
        "linker": provider_field(typing.Any, default = None),
        "linker_type": LinkerType,
        "rc_compiler": provider_field(typing.Any, default = None),
    },
)

def _legacy_equivalent_cxx_tools_info_windows(ctx: AnalysisContext, default_toolchain: CxxToolsInfo) -> CxxToolsInfo:
    return CxxToolsInfo(
        compiler = default_toolchain.compiler if ctx.attrs.compiler == None or ctx.attrs.compiler == "cl.exe" else ctx.attrs.compiler,
        compiler_type = default_toolchain.compiler_type if ctx.attrs.compiler_type == None else ctx.attrs.compiler_type,
        cxx_compiler = default_toolchain.cxx_compiler if ctx.attrs.compiler == None or ctx.attrs.compiler == "cl.exe" else ctx.attrs.compiler,
        asm_compiler = default_toolchain.asm_compiler,
        asm_compiler_type = default_toolchain.asm_compiler_type,
        rc_compiler = default_toolchain.rc_compiler if ctx.attrs.rc_compiler == None or ctx.attrs.rc_compiler == "rc.exe" else ctx.attrs.rc_compiler,
        cvtres_compiler = default_toolchain.cvtres_compiler
        if ctx.attrs.cvtres_compiler == None or ctx.attrs.cvtres_compiler == "cvtres.exe"
        else ctx.attrs.cvtres_compiler,
        archiver = default_toolchain.archiver if ctx.attrs.archiver == None else ctx.attrs.archiver,
        archiver_type = default_toolchain.archiver_type,
        linker = default_toolchain.linker if ctx.attrs.linker == None or ctx.attrs.linker == "link.exe" else ctx.attrs.linker,
        linker_type = default_toolchain.linker_type,
    )

def _legacy_equivalent_cxx_tools_info_non_windows(ctx: AnalysisContext, default_toolchain: CxxToolsInfo) -> CxxToolsInfo:
    return CxxToolsInfo(
        compiler = default_toolchain.compiler if ctx.attrs.compiler == None else ctx.attrs.compiler,
        compiler_type = default_toolchain.compiler_type if ctx.attrs.compiler_type == None else ctx.attrs.compiler_type,
        cxx_compiler = default_toolchain.cxx_compiler if ctx.attrs.cxx_compiler == None else ctx.attrs.cxx_compiler,
        asm_compiler = default_toolchain.asm_compiler if ctx.attrs.compiler == None else ctx.attrs.compiler,
        asm_compiler_type = default_toolchain.asm_compiler_type if ctx.attrs.compiler_type == None else ctx.attrs.compiler_type,
        rc_compiler = default_toolchain.rc_compiler if ctx.attrs.rc_compiler == None else ctx.attrs.rc_compiler,
        cvtres_compiler = default_toolchain.cvtres_compiler if ctx.attrs.cvtres_compiler == None else ctx.attrs.cvtres_compiler,
        archiver = default_toolchain.archiver if ctx.attrs.archiver == None else ctx.attrs.archiver,
        archiver_type = default_toolchain.archiver_type,
        linker = default_toolchain.linker if ctx.attrs.linker == None else ctx.attrs.linker,
        linker_type = default_toolchain.linker_type,
    )

def _system_cxx_toolchain_impl(ctx: AnalysisContext):
    """
    A very simple toolchain that is hardcoded to the current environment.
    """

    os = ctx.attrs._target_os_type[OsLookup].os.value
    arch_name = ctx.attrs._target_os_type[OsLookup].cpu
    cxx_tools_info = ctx.attrs._cxx_tools_info[CxxToolsInfo]
    cxx_tools_info = (
        _legacy_equivalent_cxx_tools_info_windows(ctx, cxx_tools_info)
        if os == "windows"
        else _legacy_equivalent_cxx_tools_info_non_windows(ctx, cxx_tools_info)
    )
    target_name = os
    if arch_name:
        target_name += "-" + arch_name
    return _cxx_toolchain_from_cxx_tools_info(ctx, cxx_tools_info, target_name)

def _cxx_tools_info_toolchain_impl(ctx: AnalysisContext):
    return _cxx_toolchain_from_cxx_tools_info(ctx, ctx.attrs.cxx_tools_info[CxxToolsInfo])

def _cxx_toolchain_from_cxx_tools_info(ctx: AnalysisContext, cxx_tools_info: CxxToolsInfo, target_name = "x86_64"):
    os = ctx.attrs._target_os_type[OsLookup].os
    archiver_supports_argfiles = os != Os("macos")
    additional_linker_flags = ["-fuse-ld=lld"] if os == Os("linux") and cxx_tools_info.linker != "g++" and cxx_tools_info.cxx_compiler != "g++" else []

    linker_type = cxx_tools_info.linker_type
    if os == Os("windows"):
        binary_extension = "exe"
        object_file_extension = "obj"
        static_library_extension = "lib"
        shared_library_name_default_prefix = ""
        shared_library_name_format = "{}.dll"
        shared_library_versioned_name_format = "{}.dll"
        pic_behavior = PicBehavior("not_supported")
    else:
        binary_extension = ""
        object_file_extension = "o"
        static_library_extension = "a"
        shared_library_name_default_prefix = "lib"
        shared_library_name_format = "{}.so"
        shared_library_versioned_name_format = "{}.so.{}"

        if os == Os("macos"):
            pic_behavior = PicBehavior("always_enabled")
        else:
            pic_behavior = PicBehavior("supported")

    if cxx_tools_info.compiler_type == "clang":
        llvm_link = RunInfo(args = ["llvm-link"])
    else:
        llvm_link = None

    supports_two_phase_compilation = False
    if hasattr(ctx.attrs, "supports_two_phase_compilation"):
        supports_two_phase_compilation = ctx.attrs.supports_two_phase_compilation

    if cxx_tools_info.compiler_type == "clang" or cxx_tools_info.compiler_type == "clang_cl" or cxx_tools_info.compiler_type == "clang_windows":
        cpp_dep_tracking_mode = DepTrackingMode("show_headers")
    elif cxx_tools_info.compiler_type == "windows":
        cpp_dep_tracking_mode = DepTrackingMode("show_includes")
    elif cxx_tools_info.compiler_type == "gcc":
        cpp_dep_tracking_mode = DepTrackingMode("makefile")
    else:
        cpp_dep_tracking_mode = DepTrackingMode("none")

    return [
        DefaultInfo(),
        CxxToolchainInfo(
            internal_tools = ctx.attrs.internal_tools[CxxInternalTools],
            linker_info = LinkerInfo(
                linker = _run_info(cxx_tools_info.linker),
                linker_flags = additional_linker_flags + ctx.attrs.link_flags,
                post_linker_flags = ctx.attrs.post_link_flags,
                archiver = _run_info(cxx_tools_info.archiver),
                archiver_type = cxx_tools_info.archiver_type,
                archiver_supports_argfiles = archiver_supports_argfiles,
                generate_linker_maps = False,
                lto_mode = LtoMode("none"),
                type = linker_type,
                link_binaries_locally = True,
                link_libraries_locally = True,
                archive_objects_locally = True,
                use_archiver_flags = True,
                static_dep_runtime_ld_flags = [],
                static_pic_dep_runtime_ld_flags = [],
                shared_dep_runtime_ld_flags = [],
                independent_shlib_interface_linker_flags = [],
                shlib_interfaces = ShlibInterfacesMode("disabled"),
                link_style = LinkStyle(ctx.attrs.link_style),
                link_weight = 1,
                binary_extension = binary_extension,
                object_file_extension = object_file_extension,
                shared_library_name_default_prefix = shared_library_name_default_prefix,
                shared_library_name_format = shared_library_name_format,
                shared_library_versioned_name_format = shared_library_versioned_name_format,
                static_library_extension = static_library_extension,
                force_full_hybrid_if_capable = False,
                is_pdb_generated = is_pdb_generated(linker_type, ctx.attrs.link_flags),
                link_ordering = ctx.attrs.link_ordering,
            ),
            bolt_enabled = False,
            binary_utilities_info = BinaryUtilitiesInfo(
                nm = RunInfo(args = ["nm"]),
                objcopy = RunInfo(args = ["objcopy"]),
                objdump = RunInfo(args = ["objdump"]),
                ranlib = RunInfo(args = ["ranlib"]),
                strip = RunInfo(args = ["strip"]),
                dwp = None,
            ),
            cxx_compiler_info = CxxCompilerInfo(
                compiler = _run_info(cxx_tools_info.cxx_compiler),
                preprocessor_flags = [],
                compiler_flags = ctx.attrs.cxx_flags,
                compiler_type = cxx_tools_info.compiler_type,
                supports_two_phase_compilation = supports_two_phase_compilation,
                supports_content_based_paths = ctx.attrs.supports_content_based_paths,
            ),
            c_compiler_info = CCompilerInfo(
                compiler = _run_info(cxx_tools_info.compiler),
                preprocessor_flags = [],
                compiler_flags = ctx.attrs.c_flags,
                compiler_type = cxx_tools_info.compiler_type,
                supports_content_based_paths = ctx.attrs.supports_content_based_paths,
            ),
            as_compiler_info = CCompilerInfo(
                compiler = _run_info(cxx_tools_info.compiler),
                compiler_type = cxx_tools_info.compiler_type,
                supports_content_based_paths = ctx.attrs.supports_content_based_paths,
            ),
            asm_compiler_info = CCompilerInfo(
                compiler = _run_info(cxx_tools_info.asm_compiler),
                compiler_type = cxx_tools_info.asm_compiler_type,
            ),
            cvtres_compiler_info = CvtresCompilerInfo(
                compiler = _run_info(cxx_tools_info.cvtres_compiler),
                preprocessor_flags = [],
                compiler_flags = ctx.attrs.cvtres_flags,
                compiler_type = cxx_tools_info.compiler_type,
            ),
            rc_compiler_info = RcCompilerInfo(
                compiler = _run_info(cxx_tools_info.rc_compiler),
                preprocessor_flags = [],
                compiler_flags = ctx.attrs.rc_flags,
                compiler_type = cxx_tools_info.compiler_type,
            ),
            header_mode = HeaderMode("symlink_tree_only"),
            cpp_dep_tracking_mode = cpp_dep_tracking_mode,
            pic_behavior = pic_behavior,
            llvm_link = llvm_link,
            use_dep_files = True,
            runtime_dependency_handling = RuntimeDependencyHandling("no_symlink"),
        ),
        CxxPlatformInfo(name = target_name),
    ]

def _run_info(args):
    return None if args == None else RunInfo(args = [args])

system_cxx_toolchain = rule(
    impl = _system_cxx_toolchain_impl,
    attrs = {
        "archiver": attrs.option(attrs.string(), default = None),
        "c_flags": attrs.list(attrs.arg(), default = []),
        "compiler": attrs.option(attrs.string(), default = None),
        "compiler_type": attrs.option(attrs.string(), default = None),  # One of the supported C++ compiler types.
        "cpp_dep_tracking_mode": attrs.string(default = "makefile"),
        "cvtres_compiler": attrs.option(attrs.string(), default = None),
        "cvtres_flags": attrs.list(attrs.arg(), default = []),
        "cxx_compiler": attrs.option(attrs.string(), default = None),
        "cxx_flags": attrs.list(attrs.arg(), default = []),
        "internal_tools": attrs.default_only(attrs.exec_dep(providers = [CxxInternalTools], default = "prelude//cxx/tools:internal_tools")),
        "link_flags": attrs.list(attrs.arg(), default = []),
        "link_ordering": attrs.option(attrs.enum(LinkOrdering.values()), default = None),
        "link_style": attrs.string(default = "shared"),
        "linker": attrs.option(attrs.string(), default = None),
        "post_link_flags": attrs.list(attrs.arg(), default = []),
        "rc_compiler": attrs.option(attrs.string(), default = None),
        "rc_flags": attrs.list(attrs.arg(), default = []),
        "supports_content_based_paths": attrs.bool(default = False),
        "_cxx_tools_info": attrs.exec_dep(
            providers = [CxxToolsInfo],
            default = "prelude//toolchains/msvc:msvc_tools" if host_info().os.is_windows else "prelude//toolchains/cxx/clang:path_clang_tools",
        ),
        "_target_os_type": buck.target_os_type_arg(),
    },
    is_toolchain_rule = True,
)

cxx_tools_info_toolchain = rule(
    impl = _cxx_tools_info_toolchain_impl,
    attrs = {
        "c_flags": attrs.list(attrs.arg(), default = []),
        "cpp_dep_tracking_mode": attrs.string(default = "makefile"),
        "cvtres_flags": attrs.list(attrs.arg(), default = []),
        "cxx_flags": attrs.list(attrs.arg(), default = []),
        "cxx_tools_info": attrs.exec_dep(
            providers = [CxxToolsInfo],
            default = select({
                "DEFAULT": "prelude//toolchains/cxx/clang:path_clang_tools",
                "config//os:windows": "prelude//toolchains/msvc:msvc_tools",
            }),
        ),
        "internal_tools": attrs.exec_dep(providers = [CxxInternalTools], default = "prelude//cxx/tools:internal_tools"),
        "link_flags": attrs.list(attrs.arg(), default = []),
        "link_ordering": attrs.option(attrs.enum(LinkOrdering.values()), default = None),
        "link_style": attrs.enum(
            LinkStyle.values(),
            default = "shared",
            doc = """
            The default value of the `link_style` attribute for rules that use this toolchain.
            """,
        ),
        "post_link_flags": attrs.list(attrs.arg(), default = []),
        "rc_flags": attrs.list(attrs.arg(), default = []),
        "supports_content_based_paths": attrs.bool(default = False),
        "_target_os_type": buck.target_os_type_arg(),
    },
    is_toolchain_rule = True,
)
