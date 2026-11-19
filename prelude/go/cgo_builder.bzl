# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info", "get_opt_cxx_toolchain_info")
load("@prelude//cxx:cxx_library.bzl", "cxx_compile_srcs")
load(
    "@prelude//cxx:cxx_sources.bzl",
    "CxxSrcWithFlags",
)
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo")
load(
    "@prelude//cxx:cxx_types.bzl",
    "CxxRuleConstructorParams",  # @unused Used as a type
)
load("@prelude//cxx:headers.bzl", "CxxHeadersLayout", "cxx_attr_header_namespace", "cxx_get_regular_cxx_headers_layout", "prepare_headers")
load(
    "@prelude//cxx:preprocessor.bzl",
    "CPreprocessor",
    "CPreprocessorArgs",
    "CPreprocessorInfo",
    "cxx_inherited_preprocessor_infos",
    "cxx_merge_cpreprocessors",
)
load("@prelude//cxx:target_sdk_version.bzl", "get_target_sdk_version_flags")
load("@prelude//linking:types.bzl", "Linkage")
load("@prelude//os_lookup:defs.bzl", "OsLookup")
load("@prelude//utils:cmd_script.bzl", "cmd_script")
load(":toolchain.bzl", "GoToolchainInfo", "get_toolchain_env_vars")

CGoToolOut = record(
    cgo_gotypes = field(Artifact),  # _cgo_gotypes.go
    cgo_export_h = field(Artifact),  # _cgo_export.h
    cgo_export_c = field(Artifact),  # _cgo_export.c
    cgo1_go_files = field(list[Artifact]),  # *.cgo1.go
    cgo2_c_files = field(list[Artifact]),  # *.cgo2.c
)

# Parameters extracted from ctx.attrs
CGoBuildContext = record(
    # Values from implicit attrs
    cxx_toolchain_info = field(CxxToolchainInfo),
    target_sdk_version_flags = field(list[str]),
    exec_os_type = field(OsLookup),
    # Values from explicit attrs
    header_namespace = field(str),
    headers_layout = field(CxxHeadersLayout),
    cxx_compiler_flags = field(list[typing.Any]),
    cxx_preprocessor_flags = field(list[typing.Any]),
    # Deps info
    inherited_preprocessor_infos = field(list[CPreprocessorInfo]),
    # Store "_cxx_toolchain" as "Dependency" for use in "anon_target"
    _cxx_toolchain = field(Dependency | None, None),
)

def get_cgo_build_context(ctx: AnalysisContext) -> CGoBuildContext | None:
    cxx_toolchain_info = get_opt_cxx_toolchain_info(ctx)
    if cxx_toolchain_info == None:
        return None

    return CGoBuildContext(
        cxx_toolchain_info = get_cxx_toolchain_info(ctx),
        target_sdk_version_flags = get_target_sdk_version_flags(ctx),
        exec_os_type = ctx.attrs._exec_os_type[OsLookup],
        header_namespace = cxx_attr_header_namespace(ctx),
        headers_layout = cxx_get_regular_cxx_headers_layout(ctx),
        cxx_compiler_flags = ctx.attrs.cxx_compiler_flags,
        cxx_preprocessor_flags = ctx.attrs.cxx_preprocessor_flags,
        inherited_preprocessor_infos = cxx_inherited_preprocessor_infos(ctx.attrs.deps),
        _cxx_toolchain = ctx.attrs._cxx_toolchain,
    )

_syscall_import_exclude_list = set([
    "runtime/asan",
    "runtime/cgo",
    "runtime/race",
    "runtime/msan",
])

def _cgo(
    actions: AnalysisActions,
    go_toolchain: GoToolchainInfo,
    cgo_build_context: CGoBuildContext,
    pkg_import_path: str,
    standard: bool,
    srcs: list[Artifact],
    own_pre: list[CPreprocessor],
    cgo_c_flags: list[str],
    cgo_cpp_flags: list[str],
) -> (CGoToolOut, Artifact):
    """
    Run `cgo` on `.go` sources to generate Go, C, and C-Header sources.
    """
    gen_dir = actions.declare_output("cgo_gen_tmp", dir = True, has_content_based_path = True)

    # Return a `cmd_args` to use as the generated sources.

    unimport_runtime_cgo = standard and pkg_import_path == "runtime/cgo"
    unimport_syscall = standard and pkg_import_path in _syscall_import_exclude_list

    # //line paths are resolved by the Go compiler against gen_dir.
    trimpath_replaces = cmd_args(
        [cmd_args(cmd_args(src, format = "%cwd%/{}"), cmd_args(src, relative_to = gen_dir), delimiter = "=>") for src in srcs],
        delimiter = ";",
    )

    cmd = cmd_args(
        go_toolchain.go_wrapper,
        ["--go", go_toolchain.cgo],
        "--",
        cmd_args(gen_dir.as_output(), format = "-objdir={}"),
        # todo: safe to remove once go1.28 is released
        ["-trimpath", trimpath_replaces if go_toolchain.version and go_toolchain.version.minor >= 27 else "%cwd%"],
        ["-import_runtime_cgo=false"] if unimport_runtime_cgo else [],
        ["-import_syscall=false"] if unimport_syscall else [],
        "--",
        cgo_c_flags + cgo_cpp_flags,
        cgo_build_context.cxx_compiler_flags,
        srcs,
    )

    env = get_toolchain_env_vars(go_toolchain)
    env["CC"] = _cxx_wrapper(actions, go_toolchain, cgo_build_context, own_pre)

    actions.run(cmd, env = env, category = "go_cgo", identifier = pkg_import_path, allow_cache_upload = go_toolchain.allow_cache_upload)

    return project_go_and_c_files(srcs, gen_dir), gen_dir

def project_go_and_c_files(cgo_srcs: list[Artifact], gen_dir: Artifact) -> CGoToolOut:
    return CGoToolOut(
        cgo_gotypes = gen_dir.project("_cgo_gotypes.go"),
        cgo_export_h = gen_dir.project("_cgo_export.h"),
        cgo_export_c = gen_dir.project("_cgo_export.c"),
        cgo1_go_files = [gen_dir.project(paths.replace_extension(src.basename, ".cgo1.go")) for src in cgo_srcs],
        cgo2_c_files = [gen_dir.project(paths.replace_extension(src.basename, ".cgo2.c")) for src in cgo_srcs],
    )

def _cxx_wrapper(actions: AnalysisActions, go_toolchain: GoToolchainInfo, cgo_build_context: CGoBuildContext, own_pre: list[CPreprocessor]) -> cmd_args:
    pre = cxx_merge_cpreprocessors(actions, own_pre, cgo_build_context.inherited_preprocessor_infos)
    pre_args = pre.set.project_as_args("args")
    pre_include_dirs = pre.set.project_as_args("include_dirs")

    c_compiler = cgo_build_context.cxx_toolchain_info.c_compiler_info

    # Construct the full C/C++ command needed to preprocess/compile sources.
    cxx_cmd = cmd_args(
        c_compiler.compiler,
        c_compiler.preprocessor_flags,
        cgo_build_context.target_sdk_version_flags,
        c_compiler.compiler_flags,
        pre_args,
        pre_include_dirs,
        go_toolchain.cxx_compiler_flags,
    )

    # Wrap the C/C++ command in a wrapper script to avoid arg length limits.
    return cmd_script(
        actions = actions,
        name = "cxx_wrapper",
        cmd = cxx_cmd,
        language = cgo_build_context.exec_os_type.script,
        has_content_based_path = True,
    )

# build CPreprocessor similar as cxx_private_preprocessor_info does, but with our filtered headers
def _own_pre(actions: AnalysisActions, cgo_build_context: CGoBuildContext, package_root: str, h_files: list[Artifact]) -> CPreprocessor:
    header_map = {paths.join(cgo_build_context.header_namespace, paths.relativize(h.short_path, package_root)): h for h in h_files}
    header_root = prepare_headers(actions, cgo_build_context.cxx_toolchain_info, header_map, "h_files-private-headers", uses_content_based_paths = True)

    return CPreprocessor(
        args = CPreprocessorArgs(args = ["-I", header_root.include_path] if header_root != None else []),
    )

def build_cgo(
    actions: AnalysisActions,
    target_label: Label,
    go_toolchain_info: GoToolchainInfo,
    cgo_build_context: CGoBuildContext | None,
    pkg_import_path: str,
    package_root: str,
    standard: bool,
    cgo_files: list[Artifact],
    h_files: list[Artifact],
    c_files: list[Artifact],
    c_flags: list[str],
    cpp_flags: list[str],
    anon_targets_allowed: bool = True,
) -> (list[Artifact], list[Artifact], Artifact):
    """
    Arguments:
        anon_targets_allowed: Set to `True` if the execution context allows calls to the `AnalysisActions#anon_target` API.
                              For example, this API is NOT allowed in the context of the `AnalaysisActions#dynamic_outputs` callback.
    """
    if len(cgo_files) == 0:
        return [], [], actions.copied_dir("cgo_gen_tmp", {}, has_content_based_path = True)

    if cgo_build_context == None:
        fail("cgo_build_context is None. This is likely because C++ toolchain is not available for the current target platform, but CGo files provided.")

    # Gather preprocessor inputs.
    own_pre = _own_pre(actions, cgo_build_context, package_root, h_files)

    # Separate sources into C++ and GO sources.
    cgo_tool_out, gen_dir = _cgo(actions, go_toolchain_info, cgo_build_context, pkg_import_path, standard, cgo_files, [own_pre], c_flags, cpp_flags)
    go_gen_srcs = [cgo_tool_out.cgo_gotypes] + cgo_tool_out.cgo1_go_files
    c_gen_headers = [cgo_tool_out.cgo_export_h]
    c_gen_srcs = [cgo_tool_out.cgo_export_c] + cgo_tool_out.cgo2_c_files

    # Wrap the generated CGO C headers in a CPreprocessor object for compiling.
    cgo_headers_pre = CPreprocessor(
        args = CPreprocessorArgs(
            args = [
                "-I",
                prepare_headers(
                    actions,
                    cgo_build_context.cxx_toolchain_info,
                    {h.basename: h for h in c_gen_headers},
                    "cgo-private-headers",
                    uses_content_based_paths = True,
                ).include_path,
            ]
        )
    )

    # Compile C++ sources into object files.
    c_compile_cmds = cxx_compile_srcs(
        actions,
        target_label,
        cgo_build_context.cxx_toolchain_info,
        CxxRuleConstructorParams(
            rule_type = "cgo_sources",
            headers_layout = cgo_build_context.headers_layout,
            srcs = [CxxSrcWithFlags(file = src) for src in c_files + c_gen_srcs],
            compiler_flags = go_toolchain_info.cxx_compiler_flags + c_flags + cgo_build_context.cxx_compiler_flags + cgo_build_context.target_sdk_version_flags,
            preprocessor_flags = cpp_flags + cgo_build_context.cxx_preprocessor_flags,
            anon_targets_allowed = anon_targets_allowed,
            _cxx_toolchain = cgo_build_context._cxx_toolchain,
            use_content_based_paths = True,
        ),
        # Create private header tree and propagate via args.
        [own_pre, cgo_headers_pre],
        cgo_build_context.inherited_preprocessor_infos,
        [],
        Linkage("any"),
        False,  # add_coverage_instrumentation_compiler_flags
    )

    compiled_objects = c_compile_cmds.pic.objects

    # Mark objects as requiring external linking since we're not currently supporting internal linking for CGo packages.
    dynimportfail = actions.write("dynimportfail", "", has_content_based_path = True)

    return go_gen_srcs, [dynimportfail] + compiled_objects, gen_dir
