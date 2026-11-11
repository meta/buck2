# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo", "LinkerInfo")
load(
    "@prelude//cxx:linker.bzl",
    "get_default_shared_library_name",
    "get_shared_library_name_for_param",
)
load("@prelude//linking:link_info.bzl", "LinkStrategy")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")
load("@prelude//rust/tools:attrs.bzl", "RustInternalToolsInfo")
load("@prelude//utils:cmd_script.bzl", "cmd_script")
load(":build_params.bzl", "BuildParams", "CrateType", "Emit", "MetadataKind", "ProfileMode")
load(
    ":crate_name.bzl",
    "CrateName",  # @unused Used as a type
)
load(":dep_context.bzl", "DepCollectionContext")
load(
    ":link_info.bzl",
    "resolve_deps",
)
load(
    ":rust_toolchain.bzl",
    "RustArchToolchainInfo",
    "RustToolchainInfo",
    "get_rust_arch_toolchain_info",
)
load(
    ":sources.bzl",
    "RustSources",
    "RustSourcesTSet",
    "symlinked_srcs",
)

# Struct for sharing common args between rustc and rustdoc
# (rustdoc just relays bunch of the same args to rustc when trying to gen docs)
CommonArgsInfo = record(
    args = field(cmd_args),
    subdir = field(str),
    tempfile = field(str),
    crate_type = field(CrateType),
    params = field(BuildParams),
    emit = field(Emit),
    crate_map = field(list[(CrateName, Label)]),
)

DependencyArgsInfo = record(
    args = field(cmd_args),
    crate_map = field(list[(CrateName, Label)]),
)

# Compile info which is reusable between multiple compilation command performed
# by the same rule.
CompileContext = record(
    # Clippy wrapper (wrapping clippy-driver so it has the same CLI as rustc).
    clippy_wrapper = field(cmd_args),
    # Memoized common args for reuse.
    common_args = field(dict[(CrateType, Emit, LinkStrategy, bool, bool, bool, bool, ProfileMode), CommonArgsInfo]),
    # Memoized dependency args for reuse across emits with the same inputs.
    dependency_args = field(dict[(CrateType, LinkStrategy, MetadataKind, bool), DependencyArgsInfo]),
    # Memoized environment classifications for reuse across multiple emits,
    # keyed by the environment's origin. A record field cannot be reassigned,
    # so the memo lives in a dict even while "rustc" is its only key.
    processed_env = field(dict[str, tuple]),
    cxx_toolchain_info = field(CxxToolchainInfo),
    dep_ctx = field(DepCollectionContext),
    exec_is_windows = field(bool),
    # Exec-platform OS lookup, for tools that branch on script flavor.
    exec_os_type = field(OsLookup),
    internal_tools_info = field(RustInternalToolsInfo),
    # The linker that we pass to `-Clinker`. This needs to be a wrapper script for two reasons:
    #  1. The `linker` in the cxx toolchain is a `cmd_args` which might have multiple args, we need
    #     this to be a single script.
    #  2. We need to pass some linker args that are applied before the ones rustc passes. There's
    #     unstable `-Zpre-link-arg` for that, but until that stabilizes, we pass them from within
    #     this script.
    linker_with_pre_args = field(cmd_args),
    path_sep = field(str),
    # Dylib name override, if any was provided by the target's `soname` attribute.
    soname = field(str | None),
    # Symlink root containing all sources.
    symlinked_srcs = field(Artifact),
    # All sources of transitive Rust dependencies, not including doc dependencies.
    transitive_srcs = field(RustSourcesTSet),
    # Linker args to pass the linker wrapper to rustc.
    sysroot_args = field(cmd_args),
    toolchain_info = field(RustToolchainInfo),
    arch_toolchain_info = field(RustArchToolchainInfo | None),
    transitive_dependency_dirs = field(set[Artifact]),
)

def compile_context(ctx: AnalysisContext, binary: bool = False) -> CompileContext:
    toolchain = ctx.attrs._rust_toolchain
    toolchain_info = toolchain[RustToolchainInfo]
    arch_toolchain_info = get_rust_arch_toolchain_info(toolchain, ctx.label)
    _validate_nightly_features(toolchain_info)
    internal_tools_info = ctx.attrs._rust_internal_tools_toolchain[RustInternalToolsInfo]
    cxx_toolchain_info = get_cxx_toolchain_info(ctx)

    srcs = symlinked_srcs(ctx)

    linker_with_pre_args = _linker(ctx, cxx_toolchain_info.linker_info, binary = binary)
    # Multi-arch branch dispatch directly from the variant's clippy_driver, it doesn't need the clippy_wrapper here.
    clippy_wrapper = cmd_args() if arch_toolchain_info != None else _clippy_wrapper(ctx, toolchain_info)

    dep_ctx = DepCollectionContext(
        advanced_unstable_linking = toolchain_info.advanced_unstable_linking,
        include_doc_deps = False,
        is_proc_macro = getattr(ctx.attrs, "proc_macro", False),
        explicit_sysroot_deps = toolchain_info.explicit_sysroot_deps,
        panic_runtime = toolchain_info.panic_runtime,
        linker_deps = toolchain_info.linker_deps,
    )

    transitive_srcs = ctx.actions.tset(
        RustSourcesTSet,
        value = srcs,
        children = [d.dep[RustSources].tset for d in resolve_deps(ctx, dep_ctx) if RustSources in d.dep],
    )

    # When we pass explicit sysroot deps, we need to override the default
    # sysroot to avoid accidentally linking against the prebuilt sysroot libs
    # provided by the toolchain.
    if toolchain_info.explicit_sysroot_deps:
        # Construct an empty sysroot dir with the similar structure as the real one
        # in order to appease rustc. For instance, even with -Zexternal-clangrt, rustc will emit a
        # -L{sysroot}/lib/rustlib/{target}/lib on the link line, which will fail if the sysroot dir is empty.
        empty_sysroot = ctx.actions.copied_dir(
            "empty_dir",
            {
                "lib/rustlib/{}/lib".format(toolchain_info.rustc_target_triple): ctx.actions.copied_dir("__empty__", {}, has_content_based_path = True),
            },
            has_content_based_path = True,
        )
        sysroot_args = cmd_args("--sysroot=", empty_sysroot, delimiter = "")
    elif toolchain_info.sysroot_path:
        sysroot_args = cmd_args("--sysroot=", toolchain_info.sysroot_path, delimiter = "")
    else:
        sysroot_args = cmd_args()

    exec_os_type = ctx.attrs._exec_os_type[OsLookup]
    exec_is_windows = exec_os_type.os == Os("windows")
    path_sep = "\\" if exec_is_windows else "/"

    return CompileContext(
        clippy_wrapper = clippy_wrapper,
        common_args = {},
        dependency_args = {},
        processed_env = {},
        cxx_toolchain_info = cxx_toolchain_info,
        dep_ctx = dep_ctx,
        exec_is_windows = exec_is_windows,
        exec_os_type = exec_os_type,
        internal_tools_info = internal_tools_info,
        linker_with_pre_args = linker_with_pre_args,
        path_sep = path_sep,
        soname = _attr_soname(ctx),
        symlinked_srcs = srcs,
        transitive_srcs = transitive_srcs,
        sysroot_args = sysroot_args,
        toolchain_info = toolchain_info,
        arch_toolchain_info = arch_toolchain_info,
        transitive_dependency_dirs = set(),
    )

# Reject toolchain configurations that implicitly rely on unstable compiler
# functionality if the toolchain does not also allow such functionality.
def _validate_nightly_features(toolchain_info: RustToolchainInfo):
    if toolchain_info.nightly_features:
        return

    requires_nightly = [
        ("advanced_unstable_linking", toolchain_info.advanced_unstable_linking),
        # Sysroot deps are passed via the unstable `--extern noprelude:...`
        ("explicit_sysroot_deps", toolchain_info.explicit_sysroot_deps != None),
        # `--json=unused-externs-silent` is unstable
        ("report_unused_deps", toolchain_info.report_unused_deps),
        # Custom target JSON files are unstable
        ("rust_target_path", toolchain_info.rust_target_path != None),
        # `-Zsanitizer` is unstable
        ("sanitizer", toolchain_info.sanitizer != None),
    ]
    for name, enabled in requires_nightly:
        if enabled:
            fail(
                "The Rust toolchain sets `{}`, which relies on unstable compiler functionality. It requires a toolchain with `nightly_features = True`.".format(
                    name
                )
            )

def _linker(ctx: AnalysisContext, linker_info: LinkerInfo, binary: bool = False) -> cmd_args:
    linker_flags = ctx.attrs.linker_flags
    if getattr(ctx.attrs, "_generated_build_info_enabled", False):
        linker_flags = strip_build_info_linker_flags(linker_flags)
    return cmd_script(
        actions = ctx.actions,
        name = "linker_wrapper",
        cmd = cmd_args(
            linker_info.linker,
            linker_info.linker_flags or [],
            # For "binary" rules, add C++ toolchain binary-specific linker flags.
            # TODO(agallagher): This feels a bit wrong -- it might be better to have
            # the Rust toolchain have it's own `binary_linker_flags` instead of
            # implicitly using the one from the C++ toolchain.
            linker_info.binary_linker_flags if binary else [],
            ctx.attrs._rust_toolchain[RustToolchainInfo].linker_flags,
            linker_flags,
        ),
        language = ctx.attrs._exec_os_type[OsLookup].script,
        has_content_based_path = True,
    )

def strip_build_info_linker_flags(linker_flags):
    return [flag for flag in linker_flags if not flag.startswith("--build-info")]

# Return wrapper script for clippy-driver to make sure sysroot is set right
# We need to make sure clippy is using the same sysroot - compiler, std libraries -
# as rustc itself, so explicitly invoke rustc to get the path. This is a
# (small - ~15ms per invocation) perf hit but only applies when generating
# specifically requested clippy diagnostics.
def _clippy_wrapper(ctx: AnalysisContext, toolchain_info: RustToolchainInfo) -> cmd_args:
    clippy_driver = cmd_args(toolchain_info.clippy_driver)
    rustc_print_sysroot = cmd_args(toolchain_info.compiler, "--print=sysroot", delimiter = " ")
    uses_custom_target = toolchain_info.rust_target_path != None
    if toolchain_info.rustc_target_triple:
        rustc_print_sysroot.add("--target={}".format(toolchain_info.rustc_target_triple))
        if uses_custom_target:
            rustc_print_sysroot.add("-Zunstable-options")

    skip_setting_sysroot = toolchain_info.explicit_sysroot_deps != None or toolchain_info.sysroot_path != None

    if ctx.attrs._exec_os_type[OsLookup].os == Os("windows"):
        wrapper_file, _ = ctx.actions.write(
            ctx.actions.declare_output("__clippy_driver_wrapper.bat", has_content_based_path = True),
            [
                "@echo off",
                "set __CLIPPY_INTERNAL_TESTS=true",
            ]
            + (
                [
                    "set RUSTC_BOOTSTRAP=1",
                    cmd_args(
                        toolchain_info.rust_target_path[DefaultInfo].default_outputs[0],
                        format = "set RUST_TARGET_PATH={}",
                    ),
                ]
                if uses_custom_target
                else []
            )
            + []
            + [
                cmd_args(rustc_print_sysroot, format = 'FOR /F "tokens=* USEBACKQ" %%F IN (`{}`) DO (set SYSROOT=%%F)') if not skip_setting_sysroot else "",
                cmd_args(clippy_driver, format = "{} %*"),
            ],
            allow_args = True,
            has_content_based_path = True,
        )
    else:
        wrapper_file, _ = ctx.actions.write(
            ctx.actions.declare_output("__clippy_driver_wrapper.sh", has_content_based_path = True),
            [
                "#!/usr/bin/env bash",
                # Force clippy to be clippy: https://github.com/rust-lang/rust-clippy/blob/e405c68b3c1265daa9a091ed9b4b5c5a38c0c0ba/src/driver.rs#L334
                "export __CLIPPY_INTERNAL_TESTS=true",
            ]
            + (
                [
                    "export RUSTC_BOOTSTRAP=1",
                    cmd_args(
                        toolchain_info.rust_target_path[DefaultInfo].default_outputs[0],
                        format = "export RUST_TARGET_PATH={}",
                    ),
                ]
                if uses_custom_target
                else []
            )
            + ([] if skip_setting_sysroot else [cmd_args(rustc_print_sysroot, format = "export SYSROOT=$({})")])
            + [
                cmd_args(clippy_driver, format = '{} "$@"\n'),
            ],
            is_executable = True,
            allow_args = True,
            has_content_based_path = True,
        )

    return cmd_args(wrapper_file, hidden = [clippy_driver, rustc_print_sysroot])

def _attr_soname(ctx: AnalysisContext) -> str:
    """
    Get the shared library name to set for the given rust library.
    """
    linker_info = get_cxx_toolchain_info(ctx).linker_info
    if getattr(ctx.attrs, "soname", None) != None:
        return get_shared_library_name_for_param(linker_info, ctx.attrs.soname)
    return get_default_shared_library_name(linker_info, ctx.label)

# Filenames used for various emitted forms
# `None` for a prefix or suffix means use the build_param version
_EMIT_PREFIX_SUFFIX = {
    Emit("asm"): ("", ".s"),
    Emit("llvm-bc"): ("", ".bc"),
    Emit("llvm-ir"): ("", ".ll"),
    Emit("llvm-ir-noopt"): ("", ".ll"),
    Emit("obj"): ("", ".o"),
    Emit("metadata-fast"): ("lib", ".rmeta"),  # even binaries get called 'libfoo.rmeta'
    Emit("metadata-full"): (None, None),  # Hollow rlibs, so they get the same name
    Emit("link"): (None, None),  # crate type and reloc model dependent
    Emit("rlib"): (None, None),  # crate type dependent, like `link`
    Emit("dep-info"): ("", ".d"),
    Emit("mir"): (None, ".mir"),
    Emit("expand"): (None, ".rs"),
    Emit("clippy"): ("lib", ".rmeta"),  # Treated like metadata-fast
    # Same naming as `link` (build_param prefix/suffix); the `miri`
    # subdir keeps the interpreter artifact distinct. Consumed by the
    # interpreter, never linked.
    Emit("miri"): (None, None),
}

# Return the filename for a particular emitted artifact type
def output_filename(compile_ctx: CompileContext, cratename: str, emit: Emit, buildparams: BuildParams, extra: str | None = None) -> str:
    # Allow for overriding the soname via the `soname` attribute.
    if emit == Emit("link") and buildparams.crate_type in (CrateType("dylib"), CrateType("cdylib")):
        return compile_ctx.soname

    epfx, esfx = _EMIT_PREFIX_SUFFIX[emit]
    prefix = epfx if epfx != None else buildparams.prefix
    suffix = esfx if esfx != None else buildparams.suffix
    return prefix + cratename + (extra or "") + suffix
