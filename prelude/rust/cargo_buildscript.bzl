# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Cargo build script runner compatible with Reindeer-generated targets.
#
# Use this reindeer.toml configuration to refer to this rule:
#
#     [buck]
#     buckfile_imports = """
#     load("@prelude//rust:cargo_buildscript.bzl", "buildscript_run")
#     """
#
#     # optional (this matches the default rule name):
#     buildscript_genrule = "buildscript_run"
#

load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo")
load(
    "@prelude//cxx:preprocessor.bzl",
    "cxx_inherited_preprocessor_infos",
    "cxx_merge_cpreprocessors",
)
load("@prelude//cxx:target_sdk_version.bzl", "get_target_triple")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//linking:link_info.bzl", "LinkInfosTSet", "LinkStrategy", "MergedLinkInfo")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup", "ScriptLanguage")
load("@prelude//rust:rust_toolchain.bzl", "RustToolchainInfo")
load("@prelude//rust:targets.bzl", "targets")
load("@prelude//rust/buildscript:buildscript_platform.bzl", "transition_alias")
load("@prelude//rust/tools:attrs.bzl", "RustInternalToolsInfo")
load("@prelude//utils:cmd_script.bzl", "cmd_script")
load("@prelude//utils:selects.bzl", "selects")
load(":build.bzl", "dependency_args")
load(":build_params.bzl", "MetadataKind")
load(
    ":cargo_package.bzl",
    "apply_platform_attrs",
    "get_reindeer_platform_names",
    "get_reindeer_platforms",
)
load(":dep_context.bzl", "DepCollectionContext")
load(
    ":link_info.bzl",
    "DEFAULT_STATIC_LINK_STRATEGY",
    "RustProcMacroPlugin",
    "gather_explicit_sysroot_deps",
    "resolve_rust_deps_inner",
)
load(":rust_toolchain.bzl", "PanicRuntime")

def _make_rustc_shim(ctx: AnalysisContext, cwd: Artifact) -> cmd_args:
    # Build scripts expect to receive a `rustc` which "just works." However,
    # our rustc sometimes has no sysroot available, so we need to make a shim
    # which supplies the sysroot deps if necessary
    toolchain_info = ctx.attrs._rust_toolchain[RustToolchainInfo]
    explicit_sysroot_deps = toolchain_info.explicit_sysroot_deps
    if explicit_sysroot_deps:
        dep_ctx = DepCollectionContext(
            advanced_unstable_linking = False,
            include_doc_deps = False,
            is_proc_macro = False,
            explicit_sysroot_deps = explicit_sysroot_deps,
            panic_runtime = PanicRuntime("unwind"),  # not actually used
        )
        deps = gather_explicit_sysroot_deps(dep_ctx)
        deps = resolve_rust_deps_inner(ctx, deps)
        dep_args, dep_argsfiles, _ = dependency_args(
            ctx = ctx,
            internal_tools_info = ctx.attrs._rust_internal_tools_toolchain[RustInternalToolsInfo],
            transitive_dependency_dirs = set(),
            toolchain_info = toolchain_info,
            deps = deps,
            subdir = "any",
            dep_link_strategy = DEFAULT_STATIC_LINK_STRATEGY,
            dep_metadata_kind = MetadataKind("full"),
            is_rustdoc_test = False,
            cwd = cwd,
        )

        null_path = "nul" if ctx.attrs._exec_os_type[OsLookup].os == Os("windows") else "/dev/null"
        dep_args = cmd_args("--sysroot=" + null_path, dep_args, relative_to = cwd)
        dep_file, _ = ctx.actions.write("rustc_dep_file", dep_args, allow_args = True, has_content_based_path = False)
        sysroot_args = cmd_args(
            cmd_args("@", dep_file, delimiter = "", hidden = dep_args),
            # add dep_argsfiles as a separate argument because rustc does NOT support nested @argsfiles
            dep_argsfiles,
        )
    else:
        sysroot_args = cmd_args()

    shim = cmd_script(
        actions = ctx.actions,
        name = "__rustc_shim",
        cmd = cmd_args(toolchain_info.compiler, sysroot_args, relative_to = cwd),
        language = ctx.attrs._exec_os_type[OsLookup].script,
    )

    return cmd_args(shim, relative_to = cwd)

def _make_cc_shim(ctx: AnalysisContext, name: str, cmd: cmd_args) -> cmd_args:
    """
    Wrap cmd, which can contain relative paths to executables and various
    resources, in a script that can be invoked from any directory.

    Different crates' build scripts run $CC from inside of $OUT_DIR, or from
    /tmp, not necessarily only from the directory that Buck gives to the build
    script execution. Also they pass arguments to $CC which are relative to the
    directory they chose to run it in.

    For a cmd like this:

        buck-out/v2/art/fbcode/tools/build/buck/wrappers/__fbcc__/0cdd64957fa390c4/fbcc \
        -resource-dir \
        fbcode/third-party-buck/platform010/build/llvm-fb/21/lib/clang/stable \
        -Bfbcode/third-party-buck/platform010/build/binutils/x86_64-facebook-linux/bin

    we use `absolute_prefix` to insert a recognizable marker `${..}/` in front
    of every argument that is a path. The markers are later substituted with an
    appropriate value after learning the buildscript-selected working directory.

        python3 \
        fbcode/buck2/prelude/rust/tools/from_any_dir.py \
        --cwd=/re_cwd/buck-out/v2/art/fbsource/eef091ffd45259ca/third-party/rust/vendor/gmp-mpfr-sys/__1-build-script-run__/cwd \
        ${..}/buck-out/v2/art/fbcode/tools/build/buck/wrappers/__fbcc__/0cdd64957fa390c4/fbcc \
        -resource-dir \
        ${..}/fbcode/third-party-buck/platform010/build/llvm-fb/21/lib/clang/stable \
        -B${..}/fbcode/third-party-buck/platform010/build/binutils/x86_64-facebook-linux/bin

    There are 4 categories of paths which are relative to different locations.

    1. Paths in the RunInfo of the from_any_dir tool, including the path of that
       Python program and paths in the toolchain's Python interpreter and global
       arguments to the Python interpreter. These are relative to the repo root
       and we do not prepend the marker. The wrapper script must change
       directory to the repo root before evaluating these paths.

    2. The buildscript-selected working directory. This is captured as an
       absolute path by the wrapper script and passed to from_any_dir's `--cwd`
       flag.

    3. Paths in `cmd`. We mark these for from_any_dir to rewrite relative to the
       buildscript-selected working directory. From_any_dir must change to that
       directory before evaluating these paths.

    4. Paths in the arguments passed by the build script to $CC. These are
       relative to the buildscript-selected working directory and we must have
       changed back to that directory before evaluating these paths.
    """

    internal_tools_info = ctx.attrs._rust_internal_tools_toolchain[RustInternalToolsInfo]

    language = ctx.attrs._exec_os_type[OsLookup].script
    if language == ScriptLanguage("sh"):
        script = ctx.actions.declare_output("{}.sh".format(name), has_content_based_path = True)
        wrapper, _ = ctx.actions.write(
            script,
            [
                "#!/usr/bin/env bash",
                # Capture buildscript-selected working directory.
                "cc_original_dir=$(pwd)",
                # Change directory to the script's location in buck-out, then up
                # to the repo root.
                'cd -- "$(dirname -- "$(realpath "${BASH_SOURCE[0]}")")"',
                cmd_args(ctx.label.project_root, relative_to = (script, 1), format = "cd {}"),
                # Run from_any_dir.py.
                cmd_args(
                    cmd_args(
                        cmd_args(internal_tools_info.from_any_dir, quote = "shell"),
                        '--cwd="${cc_original_dir}"',
                        cmd_args(cmd, absolute_prefix = "${..}/", quote = "shell"),
                        delimiter = " \\\n",
                    ),
                    # For linker, prepend every argument with `-Wl,`. Without this,
                    # when using Clang for the linker, arguments are intercepted
                    # by Clang instead of making it to the actual linker.
                    # >> clang++: error: unknown argument: '--as-needed'
                    format = '{} "${@/#/-Wl,}"\n' if name == "__ld_shim" else '{} "$@"\n',
                ),
            ],
            is_executable = True,
            allow_args = True,
        )
    elif language == ScriptLanguage("bat"):
        script = ctx.actions.declare_output("{}.bat".format(name), has_content_based_path = True)
        wrapper, _ = ctx.actions.write(
            script,
            [
                "@echo off",
                [
                    # Prepend every argument with `-Wl,`.
                    "setlocal enabledelayedexpansion",
                    'set "wl_args="',
                    "for %%a in (%*) do (",
                    '    set "wl_args=!wl_args! -Wl,%%~a"',
                    ")",
                ]
                if name == "__ld_shim"
                else [],
                "set cc_original_dir=%CD%",
                'cd /d "%~dp0"',
                cmd_args(ctx.label.project_root, relative_to = (script, 1), format = "cd {}"),
                cmd_args(
                    cmd_args(
                        cmd_args(internal_tools_info.from_any_dir, quote = "shell"),
                        '--cwd="%cc_original_dir%"',
                        cmd_args(cmd, absolute_prefix = "${..}\\", quote = "shell"),
                        delimiter = " ^\n  ",
                    ),
                    format = "{} !args!\n" if name == "__ld_shim" else "{} %*\n",
                ),
            ],
            is_executable = True,
            allow_args = True,
        )
    else:
        fail(language)

    return cmd_args(wrapper, hidden = [internal_tools_info.from_any_dir, cmd])

def _cargo_buildscript_impl(ctx: AnalysisContext) -> list[Provider]:
    cxx_toolchain_info = ctx.attrs._cxx_toolchain[CxxToolchainInfo]
    rust_toolchain_info = ctx.attrs._rust_toolchain[RustToolchainInfo]

    cwd = ctx.actions.declare_output("cwd", dir = True, has_content_based_path = True)
    out_dir = ctx.actions.declare_output("OUT_DIR", dir = True, has_content_based_path = True)
    rustc_flags = ctx.actions.declare_output("rustc_flags", has_content_based_path = True)

    if ctx.attrs.manifest_dir != None:
        manifest_dir = ctx.attrs.manifest_dir[DefaultInfo].default_outputs[0]
    else:
        manifest_dir = ctx.actions.symlinked_dir("manifest_dir", ctx.attrs.filegroup_for_manifest_dir, has_content_based_path = True)

    cmd = [
        ctx.attrs.runner[RunInfo],
        cmd_args("--buildscript=", ctx.attrs.buildscript[RunInfo], delimiter = ""),
        cmd_args("--rustc-cfg=", ctx.attrs.rustc_cfg[DefaultInfo].default_outputs[0], delimiter = ""),
        cmd_args("--manifest-dir=", manifest_dir, delimiter = ""),
        cmd_args("--create-cwd=", cwd.as_output(), delimiter = ""),
        cmd_args("--outfile=", rustc_flags.as_output(), delimiter = ""),
    ]

    if ctx.attrs.rustc_link_lib:
        cmd.append("--rustc-link-lib")
    if ctx.attrs.rustc_link_search:
        cmd.append("--rustc-link-search")

    # See https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-build-scripts

    env = {}
    env["CARGO"] = "/bin/false"
    env["CARGO_PKG_NAME"] = ctx.attrs.package_name
    env["CARGO_PKG_VERSION"] = ctx.attrs.version
    env["OUT_DIR"] = out_dir.as_output()
    env["RUSTC"] = _make_rustc_shim(ctx, cwd)
    env["RUSTC_LINKER"] = "/bin/false"
    env["RUST_BACKTRACE"] = "1"

    if rust_toolchain_info.rustc_target_triple:
        env["TARGET"] = rust_toolchain_info.rustc_target_triple
    else:
        cmd.append(cmd_args("--rustc-host-tuple=", ctx.attrs.rustc_host_tuple[DefaultInfo].default_outputs[0], delimiter = ""))

    # \037 == \x1f == the magic delimiter specified in the environment variable
    # reference above.
    env["CARGO_ENCODED_RUSTFLAGS"] = cmd_args(
        rust_toolchain_info.rustc_flags,
        "-Cunsafe-allow-abi-mismatch=sanitizer",
        delimiter = "\037",
    )

    host_triple = targets.exec_triple(ctx)
    if host_triple:
        env["HOST"] = host_triple

    for feature in ctx.attrs.features:
        upper_feature = feature.upper().replace("-", "_")
        env["CARGO_FEATURE_{}".format(upper_feature)] = "1"

    for flag in rust_toolchain_info.rustc_flags:
        if isinstance(flag, ResolvedStringWithMacros):
            flag = str(flag)[1:-1]
        if flag.startswith("-Copt-level="):
            opt_level = flag.removeprefix("-Copt-level=")
            if opt_level.isdigit():
                env["OPT_LEVEL"] = opt_level

    # C and C++ compilers for bindgen.
    target_triple = get_target_triple(ctx)
    preprocessor = cxx_merge_cpreprocessors(
        ctx.actions,
        [],
        cxx_inherited_preprocessor_infos(ctx.attrs.cxx_deps),
    )
    deps_preprocessor_flags = preprocessor.set.project_as_args("args")
    deps_tset = ctx.actions.tset(
        LinkInfosTSet,
        children = [dep[MergedLinkInfo]._infos[LinkStrategy("static_pic")] for dep in ctx.attrs.cxx_deps],
    )
    deps_link = deps_tset.project_as_args("default")
    sanitizer_flags = ["-fno-sanitize=all"]
    cc_is_clang = cxx_toolchain_info.c_compiler_info.compiler_type.startswith("clang")
    cxx_is_clang = cxx_toolchain_info.cxx_compiler_info.compiler_type.startswith("clang")
    env["LD"] = _make_cc_shim(
        ctx = ctx,
        name = "__ld_shim",
        cmd = cmd_args(
            cxx_toolchain_info.linker_info.linker,
            cxx_toolchain_info.linker_info.linker_flags or [],
            rust_toolchain_info.linker_flags,
            sanitizer_flags,
        ),
    )
    cc_shim = _make_cc_shim(
        ctx = ctx,
        name = "__cc_shim",
        cmd = cmd_args(
            cxx_toolchain_info.c_compiler_info.compiler,
            cmd_args(env["LD"], format = "--ld-path={}") if cc_is_clang else cmd_args(),
            cxx_toolchain_info.c_compiler_info.preprocessor_flags,
            cxx_toolchain_info.c_compiler_info.compiler_flags,
            deps_preprocessor_flags,
            deps_link,
            sanitizer_flags,
            ctx.attrs.cxx_flags,
            ["--target={}".format(target_triple)] if target_triple else [],
        ),
    )
    env["CC"] = cc_shim
    env["CXX"] = _make_cc_shim(
        ctx = ctx,
        name = "__cxx_shim",
        cmd = cmd_args(
            cxx_toolchain_info.cxx_compiler_info.compiler,
            cmd_args(env["LD"], format = "--ld-path={}") if cxx_is_clang else cmd_args(),
            cxx_toolchain_info.cxx_compiler_info.preprocessor_flags,
            cxx_toolchain_info.cxx_compiler_info.compiler_flags,
            deps_preprocessor_flags,
            deps_link,
            sanitizer_flags,
            ctx.attrs.cxx_flags,
            ["--target={}".format(target_triple)] if target_triple else [],
        ),
    )
    env["AR"] = _make_cc_shim(
        ctx = ctx,
        name = "__ar_shim",
        cmd = cmd_args(cxx_toolchain_info.linker_info.archiver),
    )

    # Environment variables specified in the target's attributes get priority
    # over all the above.
    for k, v in ctx.attrs.env.items():
        env[k] = cmd_args(v, relative_to = cwd)

    # A target that blanks CC leaves cc-rs with a `<target>-gcc` PATH lookup
    # when cross-compiling, which exists nowhere. Re-export the shim as
    # TARGET_CC, which cc-rs only consults when HOST != TARGET, below any
    # explicit CC_<target> and above CC, so native builds are unaffected.
    rust_target_triple = rust_toolchain_info.rustc_target_triple
    cc_override = ctx.attrs.env.get("CC", None)
    if isinstance(cc_override, ResolvedStringWithMacros):
        cc_override = str(cc_override)[1:-1]
    if (
        rust_target_triple != None
        and host_triple != None
        and rust_target_triple != host_triple
        and cc_override != None
        and cc_override.strip() == ""
        and "TARGET_CC" not in ctx.attrs.env
        and "CC_{}".format(rust_target_triple) not in ctx.attrs.env
        and "CC_{}".format(rust_target_triple.replace("-", "_")) not in ctx.attrs.env
    ):
        env["TARGET_CC"] = cc_shim

    ctx.actions.run(
        cmd,
        env = env,
        category = "buildscript",
    )

    return [
        DefaultInfo(
            default_output = None,
            sub_targets = {
                "out_dir": [DefaultInfo(default_output = out_dir)],
                "rustc_flags": [DefaultInfo(default_output = rustc_flags)],
            },
        )
    ]

_cargo_buildscript_rule = rule(
    impl = _cargo_buildscript_impl,
    attrs = {
        "buildscript": attrs.exec_dep(providers = [RunInfo]),
        "cxx_deps": attrs.list(attrs.dep(), default = []),
        "cxx_flags": attrs.list(attrs.arg(), default = []),
        "env": attrs.dict(key = attrs.string(), value = attrs.arg(), default = {}),
        "features": attrs.list(attrs.string(), default = []),
        "filegroup_for_manifest_dir": attrs.option(attrs.dict(key = attrs.string(), value = attrs.source()), default = None),
        "manifest_dir": attrs.option(attrs.dep(), default = None),
        "package_name": attrs.string(),
        "runner": attrs.default_only(attrs.exec_dep(providers = [RunInfo], default = "prelude//rust/tools:buildscript_run")),
        # *IMPORTANT* rustc_cfg must be a `dep` and not an `exec_dep` because
        # we want the `rustc --cfg` for the target platform, not the exec platform.
        "rustc_cfg": attrs.dep(default = "prelude//rust/tools:rustc_cfg"),
        "rustc_host_tuple": attrs.dep(default = "prelude//rust/tools:rustc_host_tuple"),
        "rustc_link_lib": attrs.bool(default = False),
        "rustc_link_search": attrs.bool(default = False),
        "version": attrs.string(),
        "_cxx_toolchain": toolchains_common.cxx(),
        "_exec_os_type": buck.exec_os_type_arg(),
        "_rust_internal_tools_toolchain": attrs.default_only(
            attrs.toolchain_dep(default = "prelude//rust/tools:internal_tools_toolchain"),
        ),
        "_rust_toolchain": toolchains_common.rust(),
    },
    # Always empty, but needed to prevent errors
    uses_plugins = [RustProcMacroPlugin],
)

def buildscript_run(
    name,
    buildscript_rule,
    package_name,
    version,
    platform = {},
    # path to crate's directory in source tree, e.g. "vendor/serde-1.0.100"
    local_manifest_dir = None,
    # target or subtarget containing crate, e.g. ":serde.git[serde]"
    manifest_dir = None,
    buildscript_compatible_with = None,
    **kwargs,
):
    kwargs = apply_platform_attrs(platform, kwargs)

    if manifest_dir == None and local_manifest_dir == None:
        existing_filegroup_name = "{}-{}.crate".format(package_name, version)
        if rule_exists(existing_filegroup_name):
            manifest_dir = ":{}".format(existing_filegroup_name)
        else:
            local_manifest_dir = "vendor/{}-{}".format(package_name, version)

    filegroup_for_manifest_dir = None
    if local_manifest_dir != None:
        prefix_with_trailing_slash = "" if local_manifest_dir in ("", ".") else "{}/".format(local_manifest_dir)
        filegroup_for_manifest_dir = {path.removeprefix(prefix_with_trailing_slash): path for path in glob(["{}**".format(prefix_with_trailing_slash)])}

    def platform_buildscript_build_name(plat):
        if name.endswith("-build-script-run"):
            # This is the expected case for Reindeer-generated targets, which
            # come in pairs build-script-run and build-script-build.
            return "{}-build-script-build-{}".format(
                name.removesuffix("-build-script-run"),
                plat,
            )
        else:
            return "{}-{}".format(name, plat)

    for i, plat in enumerate(get_reindeer_platform_names()):
        transition_alias(
            name = platform_buildscript_build_name(plat),
            actual = buildscript_rule,
            incoming_transition = "prelude//rust/buildscript:buildscript_platform_transition[{}]".format(i),
            target_compatible_with = buildscript_compatible_with,
            visibility = [],
        )

    buildscript_rule = selects.apply(
        get_reindeer_platforms(),
        lambda plat: buildscript_rule if plat == None else ":{}".format(platform_buildscript_build_name(plat)),
    )

    _cargo_buildscript_rule(
        name = name,
        buildscript = buildscript_rule,
        package_name = package_name,
        version = version,
        filegroup_for_manifest_dir = filegroup_for_manifest_dir,
        manifest_dir = manifest_dir,
        **kwargs,
    )
