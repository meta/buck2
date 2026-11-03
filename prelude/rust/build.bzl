# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactTSet",
    "make_artifact_tset",
    "project_artifacts",
)
load("@prelude//:local_only.bzl", "link_cxx_binary_locally")
load("@prelude//:paths.bzl", "paths")
load("@prelude//:resources.bzl", "create_resource_db", "gather_resources")
load(
    "@prelude//apple:apple_frameworks.bzl",
    "apple_build_link_args_with_deduped_flags",
    "apple_get_link_info_by_deduping_link_infos",
)
load("@prelude//cxx:archive.bzl", "archive_flags")
load("@prelude//cxx:cxx_library_utility.bzl", "cxx_attr_deps")
load(
    "@prelude//cxx:cxx_link_utility.bzl",
    "executable_shared_lib_arguments",
    "make_link_args",
)
load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "LinkerType",
)
load("@prelude//cxx:debug.bzl", "SplitDebugMode")
load("@prelude//cxx:dwp.bzl", "dwp", "dwp_available")
load(
    "@prelude//cxx:link.bzl",
    "CxxLinkResult",  # @unused Used as a type
    "cxx_link_into",
    "cxx_link_shared_library",
)
load(
    "@prelude//cxx:link_types.bzl",
    "CxxLinkResultType",
    "link_options",
)
load(
    "@prelude//cxx:linker.bzl",
    "get_import_library",
    "get_shared_library_name_linker_flags",
)
load(
    "@prelude//cxx:transformation_spec.bzl",
    "TransformationSpecContext",  # @unused Used as a type
)
load(
    "@prelude//linking:execution_preference.bzl",
    "LinkExecutionPreference",
    "get_link_execution_preference",
)
load(
    "@prelude//linking:link_info.bzl",
    "Archive",
    "ArchiveLinkable",
    "LibOutputStyle",  # @unused Used as a type
    "LinkArgs",
    "LinkInfo",
    "LinkInfos",  # @unused Used as a type
    "LinkStrategy",
    "LinkedObject",  # @unused Used as a type
    "create_merged_link_info",
    "get_link_args_for_strategy",
    "set_link_info_link_whole",
)
load("@prelude//linking:lto.bzl", "LtoMode")
load(
    "@prelude//linking:shared_libraries.bzl",
    "merge_shared_libraries",
)
load("@prelude//linking:strip.bzl", "strip_debug_info")
load("@prelude//linking:types.bzl", "Linkage")
load("@prelude//os_lookup:defs.bzl", "OsLookup")
load("@prelude//rust/tools:attrs.bzl", "RustInternalToolsInfo")
load("@prelude//utils:argfile.bzl", "at_argfile")
load("@prelude//utils:cmd_script.bzl", "cmd_script")
load(
    "@prelude//utils:utils.bzl",
    "flatten_dict",
)
load(
    ":build_params.bzl",
    "BuildParams",  # @unused Used as a type
    "CrateType",
    "Emit",
    "MetadataKind",
    "ProfileMode",  # @unused Used as a type
    "RelocModel",
    "crate_type_linked",
    "dep_metadata_of_emit",
)
load(":clippy_configuration.bzl", "ClippyConfiguration")
load(
    ":context.bzl",
    "CommonArgsInfo",
    "CompileContext",
    "DependencyArgsInfo",
    "output_filename",
    "strip_build_info_linker_flags",
)
load(
    ":crate_name.bzl",
    "CrateName",  # @unused Used as a type
)
load(":dep_context.bzl", "DepCollectionContext")
load(
    ":extern.bzl",
    "crate_map_arg",
    "extern_arg",
)
load(
    ":failure_filter.bzl",
    "failure_filter",
)
load(
    ":link_info.bzl",
    "RustArtifact",
    "RustCxxLinkGroupInfo",  # @unused Used as a type
    "RustDependency",
    "RustProcMacroInfo",
    "TransitiveDeps",
    "attr_crate",
    "attr_simple_crate_for_filenames",
    "executable_shared_lib_arguments_from_shared_library_info",
    "get_available_proc_macros",
    "inherited_external_debug_info",
    "inherited_merged_link_infos",
    "inherited_shared_libs",
    "normalize_crate",
    "resolve_deps",
    "resolve_rust_deps",
    "strategy_info",
)
load(
    ":outputs.bzl",
    "LinkExtraction",
    "RustcCompileOutput",
    "RustcLinkedOutput",
    "RustcOutput",
    "RustcUnlinkedOutput",
)
load(":resources.bzl", "rust_attr_resources")
load(":rust_toolchain.bzl", "PanicRuntime", "RustToolchainInfo")
load(":action_adapter.bzl", "adapt_rustc_action")
load(
    ":sources.bzl",
    "RustSources",
    "RustSourcesTSet",
)

def generate_rustdoc(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    # link style doesn't matter, but caller should pass in build params
    # with static-pic (to get best cache hits for deps)
    params: BuildParams,
    default_roots: list[str],
    document_private_items: bool,
) -> Artifact:
    toolchain_info = compile_ctx.toolchain_info

    common_args = _compute_common_args(
        ctx = ctx,
        compile_ctx = compile_ctx,
        dep_ctx = compile_ctx.dep_ctx,
        # to make sure we get the rmeta's generated for the crate dependencies,
        # rather than full .rlibs
        emit = Emit("metadata-fast"),
        params = params,
        default_roots = default_roots,
        infallible_diagnostics = False,
        incremental_enabled = False,
        is_rustdoc_test = False,
        profile_mode = None,
    )

    subdir = common_args.subdir + "-rustdoc"
    use_cbp = getattr(ctx.attrs, "use_content_based_paths", False)
    output = ctx.actions.declare_output(subdir, has_content_based_path = use_cbp)

    plain_env, path_env = process_env(compile_ctx, toolchain_info.rustdoc_env | ctx.attrs.env)
    plain_env["RUSTDOC_BUCK_TARGET"] = cmd_args(str(ctx.label.raw_target()))

    # The toolchain's `rustdoc_flags` may rely on unstable functionality, e.g.
    # `--generate-link-to-definition`.
    if toolchain_info.nightly_features and "RUSTC_BOOTSTRAP" not in plain_env:
        plain_env["RUSTC_BOOTSTRAP"] = cmd_args("1")

    if toolchain_info.rust_target_path != None:
        path_env["RUST_TARGET_PATH"] = toolchain_info.rust_target_path[DefaultInfo].default_outputs[0]

    rustdoc_cmd = cmd_args(
        toolchain_info.rustdoc,
        "--rustc-action-separator",
        toolchain_info.rustdoc_flags,
        ctx.attrs.rustdoc_flags,
        common_args.args,
        cmd_args(output.as_output(), format = "--out-dir={}"),
        hidden = [toolchain_info.rustdoc, compile_ctx.symlinked_srcs],
    )

    if document_private_items:
        rustdoc_cmd.add("--document-private-items")

    rustdoc_cmd_action = cmd_args(
        [cmd_args("--env=", k, "=", v, delimiter = "") for k, v in plain_env.items()],
        [cmd_args("--path-env=", k, "=", v, delimiter = "") for k, v in path_env.items()],
        rustdoc_cmd,
    )

    rustdoc_cmd = _long_command(
        ctx = ctx,
        exe = compile_ctx.internal_tools_info.rustc_action,
        args = rustdoc_cmd_action,
        argfile_name = "{}.args".format(subdir),
        has_content_based_path = use_cbp,
    )

    ctx.actions.run(rustdoc_cmd, category = "rustdoc")

    return output

def generate_rustdoc_coverage(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    # link strategy doesn't matter, but caller should pass in build params
    # with static-pic (to get best cache hits for deps)
    params: BuildParams,
    default_roots: list[str],
) -> Artifact:
    toolchain_info = compile_ctx.toolchain_info

    if not toolchain_info.nightly_features:
        fail("Doc coverage uses the unstable `--show-coverage` flag and requires a toolchain with `nightly_features = True`")

    common_args = _compute_common_args(
        ctx = ctx,
        compile_ctx = compile_ctx,
        dep_ctx = compile_ctx.dep_ctx,
        # to make sure we get the rmeta's generated for the crate dependencies,
        # rather than full .rlibs
        emit = Emit("metadata-fast"),
        params = params,
        default_roots = default_roots,
        infallible_diagnostics = False,
        incremental_enabled = False,
        is_rustdoc_test = False,
        profile_mode = None,
    )

    file = common_args.subdir + "-rustdoc-coverage"
    use_cbp = getattr(ctx.attrs, "use_content_based_paths", False)
    output = ctx.actions.declare_output(file, has_content_based_path = use_cbp)

    plain_env, path_env = process_env(compile_ctx, ctx.attrs.env)
    plain_env["RUSTDOC_BUCK_TARGET"] = cmd_args(str(ctx.label.raw_target()))

    if toolchain_info.rust_target_path != None:
        path_env["RUST_TARGET_PATH"] = toolchain_info.rust_target_path[DefaultInfo].default_outputs[0]

    # `--show-coverage` is unstable.
    plain_env["RUSTC_BOOTSTRAP"] = cmd_args("1")
    unstable_options = ["-Zunstable-options"]

    rustdoc_cmd_action = cmd_args(
        [cmd_args("--env=", k, "=", v, delimiter = "") for k, v in plain_env.items()],
        [cmd_args("--path-env=", k, "=", v, delimiter = "") for k, v in path_env.items()],
        toolchain_info.rustdoc,
        "--rustc-action-separator",
        toolchain_info.rustdoc_flags,
        ctx.attrs.rustdoc_flags,
        common_args.args,
        unstable_options,
        "--show-coverage",
    )

    rustdoc_cmd = _long_command(
        ctx = ctx,
        exe = compile_ctx.internal_tools_info.rustc_action,
        args = rustdoc_cmd_action,
        argfile_name = "{}.args".format(file),
        has_content_based_path = use_cbp,
    )

    cmd = cmd_args([compile_ctx.internal_tools_info.rustdoc_coverage, output.as_output(), rustdoc_cmd])

    ctx.actions.run(cmd, category = "rustdoc_coverage")

    return output

def generate_rustdoc_test(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    rlib: Artifact,
    link_infos: dict[LibOutputStyle, LinkInfos],
    params: BuildParams,
    default_roots: list[str],
) -> cmd_args:
    toolchain_info = compile_ctx.toolchain_info

    if not toolchain_info.nightly_features:
        fail("Doctests use the unstable `--test-runtool` flag and require a toolchain with `nightly_features = True`")

    internal_tools_info = compile_ctx.internal_tools_info
    doc_dep_ctx = DepCollectionContext(
        advanced_unstable_linking = compile_ctx.dep_ctx.advanced_unstable_linking,
        include_doc_deps = True,
        is_proc_macro = False,
        explicit_sysroot_deps = compile_ctx.dep_ctx.explicit_sysroot_deps,
        panic_runtime = compile_ctx.dep_ctx.panic_runtime,
        linker_deps = compile_ctx.dep_ctx.linker_deps,
    )
    transitive_srcs = ctx.actions.tset(
        RustSourcesTSet,
        value = compile_ctx.symlinked_srcs,
        children = [d.dep[RustSources].tset for d in resolve_deps(ctx, doc_dep_ctx) if RustSources in d.dep],
    )

    resources = create_resource_db(
        ctx = ctx,
        name = "doctest/resources.json",
        binary = rlib,
        resources = flatten_dict(
            gather_resources(
                label = ctx.label,
                resources = rust_attr_resources(ctx),
                deps = cxx_attr_deps(ctx),
            ).values()
        ),
    )

    # Gather and setup symlink tree of transitive shared library deps.
    if params.dep_link_strategy == LinkStrategy("shared"):
        shlib_info = merge_shared_libraries(
            ctx.actions,
            deps = inherited_shared_libs(ctx, doc_dep_ctx),
        )
        executable_args = executable_shared_lib_arguments_from_shared_library_info(
            ctx,
            compile_ctx.cxx_toolchain_info,
            compile_ctx.internal_tools_info,
            resources,
            shlib_info,
        )
    else:
        executable_args = executable_shared_lib_arguments(
            ctx,
            compile_ctx.cxx_toolchain_info,
            resources,
            shared_libs = [],
        )

    common_args = _compute_common_args(
        ctx = ctx,
        compile_ctx = compile_ctx,
        dep_ctx = doc_dep_ctx,
        emit = Emit("link"),
        params = params,
        default_roots = default_roots,
        infallible_diagnostics = False,
        is_rustdoc_test = True,
        incremental_enabled = False,
        profile_mode = None,
    )

    link_args_output = make_link_args(
        ctx,
        ctx.actions,
        compile_ctx.cxx_toolchain_info,
        [
            LinkArgs(flags = executable_args.extra_link_args),
            get_link_args_for_strategy(
                ctx.actions,
                ctx.label,
                compile_ctx.cxx_toolchain_info.linker_info,
                # Since we pass the rlib in and treat it as a dependency to the rustdoc test harness,
                # we need to ensure that the rlib's link info is added to the linker, otherwise we may
                # end up with missing symbols that are defined within the crate.
                [
                    create_merged_link_info(
                        ctx,
                        compile_ctx.cxx_toolchain_info.pic_behavior,
                        link_infos,
                        deps = inherited_merged_link_infos(ctx, doc_dep_ctx),
                        preferred_linkage = Linkage("static"),
                    )
                ]
                + inherited_merged_link_infos(ctx, doc_dep_ctx),
                params.dep_link_strategy,
                prefer_stripped = False,
                transformation_spec_context = None,
            ),
        ],
    )

    link_args_output.link_args.add(ctx.attrs.doc_linker_flags or [])

    linker_argsfile, _ = ctx.actions.write(
        "{}/__{}_linker_args.txt".format(common_args.subdir, common_args.tempfile),
        link_args_output.link_args,
        allow_args = True,
        has_content_based_path = False,
    )

    if compile_ctx.exec_is_windows:
        runtool = ["--test-runtool=cmd.exe", "--test-runtool-arg=/V:OFF", "--test-runtool-arg=/C"]
    else:
        runtool = ["--test-runtool=/usr/bin/env"]

    plain_env, path_env = process_env(compile_ctx, ctx.attrs.env)
    doc_plain_env, doc_path_env = process_env(compile_ctx, ctx.attrs.doc_env)
    for k, v in doc_plain_env.items():
        path_env.pop(k, None)
        plain_env[k] = v
    for k, v in doc_path_env.items():
        plain_env.pop(k, None)
        path_env[k] = v

    # `--test-runtool` is unstable.
    plain_env["RUSTC_BOOTSTRAP"] = cmd_args("1")
    unstable_options = ["-Zunstable-options"]

    if toolchain_info.rust_target_path != None:
        path_env["RUST_TARGET_PATH"] = toolchain_info.rust_target_path[DefaultInfo].default_outputs[0]

    rustdoc_cmd = cmd_args(
        [cmd_args("--env=", k, "=", v, delimiter = "") for k, v in plain_env.items()],
        [cmd_args("--path-env=", k, "=", v, delimiter = "") for k, v in path_env.items()],
        toolchain_info.rustdoc,
        "--rustc-action-separator",
        "--test",
        unstable_options,
        cmd_args("--test-builder=", toolchain_info.compiler, delimiter = ""),
        toolchain_info.rustdoc_flags,
        ctx.attrs.rustdoc_flags,
        common_args.args,
        extern_arg([], attr_crate(ctx), rlib),
        "--extern=proc_macro" if ctx.attrs.proc_macro else [],
        cmd_args(compile_ctx.linker_with_pre_args, format = "-Clinker={}"),
        cmd_args(linker_argsfile, format = "-Clink-arg=@{}"),
        runtool,
        cmd_args(internal_tools_info.rustdoc_test_with_resources, format = "--test-runtool-arg={}"),
        cmd_args("--test-runtool-arg=--resources=", resources, delimiter = ""),
        "--color=always",
        "--test-args=--color=always",
        cmd_args(
            "--remap-path-prefix=",
            compile_ctx.symlinked_srcs,
            compile_ctx.path_sep,
            "=",
            compile_ctx.symlinked_srcs.owner.path,
            compile_ctx.path_sep,
            delimiter = "",
        ),
        hidden = [
            transitive_srcs.project_as_args("artifacts"),
            link_args_output.hidden,
            executable_args.runtime_files,
        ],
    )

    return _long_command(
        ctx = ctx,
        exe = internal_tools_info.rustc_action,
        args = rustdoc_cmd,
        argfile_name = "{}.args".format(common_args.subdir),
        has_content_based_path = getattr(ctx.attrs, "use_content_based_paths", False),
    )

def _archiver_command(ctx: AnalysisContext, compile_ctx: CompileContext, subdir: str, archive_cbp: bool) -> cmd_args:
    linker_info = compile_ctx.cxx_toolchain_info.linker_info
    archiver_type = linker_info.archiver_type

    if archiver_type not in ["gnu", "llvm", "bsd"]:
        fail("archiving rust link objects is unsupported for archiver type '{}'".format(archiver_type))

    archiver_cmd = cmd_args(
        linker_info.archiver,
        archive_flags(
            archiver_type,
            linker_info.type,
            linker_info.use_archiver_flags,
            linker_info.archive_symbol_table,
            thin = False,
        ),
    )
    if linker_info.use_archiver_flags and linker_info.archiver_flags != None:
        archiver_cmd.add(linker_info.archiver_flags)

    argsfile, _ = ctx.actions.write(
        subdir + "/archiver.args",
        archiver_cmd,
        allow_args = True,
        has_content_based_path = archive_cbp,
    )
    return cmd_args(argsfile, hidden = linker_info.archiver)

def _setup_link_extraction(ctx: AnalysisContext, compile_ctx: CompileContext, subdir: str, emit_cbp: bool, archive_objects: bool) -> LinkExtraction:
    out_argsfile = ctx.actions.declare_output(subdir + "/extracted-link-args.args", has_content_based_path = emit_cbp)
    out_artifacts_dir = ctx.actions.declare_output(subdir + "/extracted-link-artifacts", dir = True, has_content_based_path = emit_cbp)
    out_manifest = ctx.actions.declare_output(subdir + "/extracted-link-manifest.txt", has_content_based_path = emit_cbp)
    linker_cmd = cmd_args(
        compile_ctx.internal_tools_info.extract_link_action,
        cmd_args(out_argsfile.as_output(), format = "--out_argsfile={}"),
        cmd_args(out_artifacts_dir.as_output(), format = "--out_artifacts={}"),
        cmd_args(out_manifest.as_output(), format = "--out_manifest={}"),
    )

    out_archive = None
    if archive_objects:
        archive_cbp = compile_ctx.cxx_toolchain_info.linker_info.supports_content_based_paths_for_archiving == True
        out_archive = ctx.actions.declare_output(subdir + "/extracted-objects.a", has_content_based_path = archive_cbp)
        linker_cmd.add(cmd_args(out_archive.as_output(), format = "--out_archive={}"))
        linker_cmd.add(cmd_args(_archiver_command(ctx, compile_ctx, subdir, archive_cbp), format = "--archiver_argsfile={}"))

    linker_cmd.add(compile_ctx.linker_with_pre_args)
    linker_wrapper = cmd_script(
        actions = ctx.actions,
        name = subdir + "/linker_wrapper",
        cmd = linker_cmd,
        language = ctx.attrs._exec_os_type[OsLookup].script,
        has_content_based_path = True,
    )
    return LinkExtraction(
        linker_wrapper = linker_wrapper,
        out_argsfile = out_argsfile,
        out_artifacts_dir = out_artifacts_dir,
        out_manifest = out_manifest,
        out_archive = out_archive,
    )

# Command interpreting the root crate under Miri, for `[miri]` test runs and
# `buck2 run`. Built like `generate_rustdoc_test`: the root crate's ordinary
# compile args, wrapped in `rustc_action` like the doctest command.
# Whatever the native compile gets (`--cfg`s, `-C` flags, lints) the
# interpreted one gets too, so the two cannot diverge.
def miri_command(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    params: BuildParams,
    default_roots: list[str],
    extra_flags: list[str | ResolvedStringWithMacros | Artifact],
    # Env of the interpreted program; the driver process's env (`env!`)
    # rides along as wrapper `--env`/`--path-env` below.
    runtime_env: dict[str, str | ResolvedStringWithMacros | Artifact],
    is_test: bool,
) -> cmd_args:
    toolchain_info = compile_ctx.toolchain_info
    if toolchain_info.miri_driver == None:
        fail("`miri_command` requires a toolchain with `miri_driver`")

    common_args = _compute_common_args(
        ctx = ctx,
        compile_ctx = compile_ctx,
        dep_ctx = compile_ctx.dep_ctx,
        emit = Emit("miri"),
        params = params,
        default_roots = default_roots,
        infallible_diagnostics = False,
        incremental_enabled = False,
        is_rustdoc_test = False,
        is_miri_run = True,
        profile_mode = None,
    )

    # Compile-time env processed like normal rustc actions, so `env!` matches.
    # The wrapper sets it on the driver process, absolutizing paths.
    plain_env, path_env = process_env(compile_ctx, toolchain_info.rustc_env | ctx.attrs.env)
    if toolchain_info.nightly_features and "RUSTC_BOOTSTRAP" not in plain_env:
        plain_env["RUSTC_BOOTSTRAP"] = cmd_args("1")
    if toolchain_info.rust_target_path != None:
        path_env["RUST_TARGET_PATH"] = toolchain_info.rust_target_path[DefaultInfo].default_outputs[0]

    internal_tools_info = compile_ctx.internal_tools_info

    # Never written, but the driver validates `-o` like rustc: without it the
    # output would be `<crate name>` in the cwd, which may be an existing
    # directory. The path must not exist: an existing read-only file fails the
    # driver's writability check.
    miri_out_placeholder = ctx.actions.write(ctx.actions.declare_output("__miri_out", "placeholder"), [""])
    miri_out = cmd_args(miri_out_placeholder, parent = 1, format = "{}" + compile_ctx.path_sep + "out")

    miri_cmd = cmd_args(
        [cmd_args("--env=", k, "=", v, delimiter = "") for (k, v) in plain_env.items()],
        [cmd_args("--path-env=", k, "=", v, delimiter = "") for (k, v) in path_env.items()],
        # Rendered diagnostics stream straight to stderr, unparsed.
        "--passthrough-stderr",
        # Driver plus its flags, kept raw: expansion applies only after
        # `--rustc-action-separator`, so literal `$(...)` survives.
        toolchain_info.miri_driver,
        toolchain_info.miri_flags,
        # Miri flags, most-specific-last so the invocation wins on conflicts:
        # toolchain defaults, target `miri_flags`, `-c rust.extra_miri_flags`.
        ctx.attrs.miri_flags,
        toolchain_info.extra_miri_flags,
        # The driver's environment is compile time (`env!`); the interpreted
        # program's comes through Miri.
        [cmd_args("-Zmiri-env-set=", k, "=", v, delimiter = "") for (k, v) in runtime_env.items()],
        "--rustc-action-separator",
        # Same order as `rust_compile`: lints first so later args override.
        _lint_flags(compile_ctx, infallible_diagnostics = False, is_clippy = False),
        common_args.args,
        "-o",
        miri_out,
        cmd_args(
            "--remap-path-prefix=",
            compile_ctx.symlinked_srcs,
            compile_ctx.path_sep,
            "=",
            compile_ctx.symlinked_srcs.owner.path,
            compile_ctx.path_sep,
            delimiter = "",
        ),
        ["-Zremap-cwd-prefix=."] if toolchain_info.nightly_features else [],
        extra_flags,
        # Last, where `miri`-kind library compiles put them.
        toolchain_info.miri_rustc_flags,
    )

    # With an abort runtime, `-Zpanic-abort-tests` makes libtest spawn a
    # subprocess per test, which the interpreter cannot. Force in-process
    # runs from the program tail: in the driver prefix Miri/rustc would
    # consume the flag instead of libtest.
    program_args = []
    if is_test and toolchain_info.panic_runtime == PanicRuntime("abort"):
        program_args = ["--force-run-in-process"]

    wrapped_command = _long_command(
        ctx = ctx,
        exe = internal_tools_info.rustc_action,
        args = miri_cmd,
        argfile_name = "{}.args".format(common_args.subdir),
        has_content_based_path = getattr(ctx.attrs, "use_content_based_paths", False),
    )

    # The boundary stays outside the response file (`_long_command` always
    # writes one); baked program arguments precede the caller's.
    return cmd_args(wrapped_command, "--program-args", program_args)

# Generate a compilation action. A single instance of rustc can emit
# numerous output artifacts, so return an artifact object for each of
# them.
def rust_compile(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    emit: Emit,
    params: BuildParams,
    default_roots: list[str],
    incremental_enabled: bool,
    extra_link_args: list[typing.Any] = [],
    predeclared_output: Artifact | None = None,
    # Whether `predeclared_output` was declared with a content-based path. The
    # linker writes its side outputs (the `.pdb` on Windows) beside the binary,
    # so they must be declared with the binary's path mode to be found.
    predeclared_output_has_content_based_path: bool = False,
    extra_flags: list[str | ResolvedStringWithMacros | Artifact] = [],
    allow_cache_upload: bool = False,
    # Setting this to true causes the diagnostic outputs that are generated
    # from this action to always be successfully generated, even if
    # compilation fails. This should not generally be used if the "real"
    # output of the action is going to be depended on
    infallible_diagnostics: bool = False,
    transformation_spec_context: TransformationSpecContext | None = None,
    profile_mode: ProfileMode | None = None,
    precomputed_inherited_link_args: LinkArgs | None = None,
) -> RustcOutput:
    toolchain_info = compile_ctx.toolchain_info

    if not toolchain_info.nightly_features:
        if emit == Emit("expand"):
            fail("Expansion uses the unstable `-Zunpretty` flag and requires a toolchain with `nightly_features = True`")
        if emit == Emit("miri"):
            fail("Miri uses unstable `-Z` flags and requires a toolchain with `nightly_features = True`")
        if profile_mode in (ProfileMode("llvm-time-trace"), ProfileMode("self-profile")):
            fail("`{}` profiling uses unstable `-Z` flags and requires a toolchain with `nightly_features = True`".format(profile_mode.value))

    lints = _lint_flags(compile_ctx, infallible_diagnostics, emit == Emit("clippy"))

    common_args = _compute_common_args(
        ctx = ctx,
        compile_ctx = compile_ctx,
        dep_ctx = compile_ctx.dep_ctx,
        emit = emit,
        params = params,
        default_roots = default_roots,
        infallible_diagnostics = infallible_diagnostics,
        incremental_enabled = incremental_enabled,
        is_rustdoc_test = False,
        profile_mode = profile_mode,
    )

    requires_linking = crate_type_linked(params.crate_type) and emit == Emit("link")
    extracts_objects = emit == Emit("rlib") and params.crate_type == CrateType("bin")

    if (
        emit == Emit("link")
        and compile_ctx.toolchain_info.advanced_unstable_linking
        # FIXME(JakobDegen): We should probably not support cdylib or staticlib under AUL
        and params.crate_type not in [CrateType("rlib"), CrateType("proc-macro"), CrateType("staticlib"), CrateType("cdylib")]
    ):
        fail('rustc-driven linking is not supported with `advanced_unstable_linking`: binaries compile via `Emit("rlib")` and link through cxx')

    if extracts_objects:
        if not compile_ctx.toolchain_info.advanced_unstable_linking:
            fail(
                '`Emit("rlib")` for a bin crate requires `advanced_unstable_linking`: without it, dependency link providers do not carry the rlibs for cxx to link'
            )
        if predeclared_output != None:
            fail("extraction produces no linked output; the caller owns the linked artifact and must produce it via `rust_link_binary`")

    use_cbp = getattr(ctx.attrs, "use_content_based_paths", False)
    emit_cbp = use_cbp if predeclared_output == None else predeclared_output_has_content_based_path

    rustc_cmd = cmd_args(
        # Lints go first to allow other args to override them.
        lints,
        getattr(ctx.attrs, "clippy_flags", []) if emit == Emit("clippy") else [],
        # Report unused --extern crates in the notification stream.
        ["--json=unused-externs-silent", "-Wunused-crate-dependencies"] if toolchain_info.report_unused_deps else [],
        common_args.args,
        cmd_args(
            "--remap-path-prefix=",
            compile_ctx.symlinked_srcs,
            compile_ctx.path_sep,
            "=",
            compile_ctx.symlinked_srcs.owner.path,
            compile_ctx.path_sep,
            delimiter = "",
        ),
        ["-Zremap-cwd-prefix=."] if toolchain_info.nightly_features else [],
        extra_flags,
    )

    if emit == Emit("clippy"):
        rustc_bin = compile_ctx.clippy_wrapper
    elif emit == Emit("miri"):
        # The driver running as rustc (`MIRI_BE_RUSTC=target`, set below)
        # prepends the flags its own version needs baked into dependency MIR
        # (`-Zmir-opt-level=0`, `-Zmir-preserve-ub`, ...), so toolchains cannot
        # drift from the interpreter by hand-copying that list.
        if toolchain_info.miri_driver == None:
            fail('`Emit("miri")` requires a toolchain with `miri_driver`')
        rustc_bin = toolchain_info.miri_driver
    else:
        rustc_bin = toolchain_info.compiler

    # If we're using failure filtering then we need to make sure the final
    # artifact location is the predeclared one since its specific path may have
    # already been encoded into the other compile args (eg rpath). So we still
    # let rustc_emit generate its own output artifacts, and then make sure we
    # use the predeclared one as the output after the failure filter action
    # below. Otherwise we'll use the predeclared outputs directly.
    if infallible_diagnostics:
        emit_op = _rustc_emit(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = emit,
            subdir = common_args.subdir,
            params = params,
            incremental_enabled = incremental_enabled,
            profile_mode = profile_mode,
        )
    else:
        emit_op = _rustc_emit(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = emit,
            subdir = common_args.subdir,
            params = params,
            predeclared_output = predeclared_output,
            incremental_enabled = incremental_enabled,
            profile_mode = profile_mode,
        )

    if emit == Emit("clippy"):
        clippy_toml = None
        if ctx.attrs.clippy_configuration:
            clippy_toml = ctx.attrs.clippy_configuration[ClippyConfiguration].clippy_toml
        elif toolchain_info.clippy_toml:
            clippy_toml = toolchain_info.clippy_toml

        if clippy_toml:
            # Clippy wants to be given a path to a directory containing a
            # clippy.toml (or .clippy.toml). Our buckconfig accepts an arbitrary
            # label like //path/to:my-clippy.toml which may not have the
            # filename that clippy looks for. Here we make a directory that
            # symlinks the requested configuration file under the required name.
            clippy_conf_dir = ctx.actions.symlinked_dir(
                common_args.subdir + "-clippy-configuration",
                {"clippy.toml": clippy_toml},
                has_content_based_path = use_cbp,
            )
            emit_op.env["CLIPPY_CONF_DIR"] = clippy_conf_dir

    if emit == Emit("miri"):
        emit_op.env["MIRI_BE_RUSTC"] = "target"

    split_debug_mode = compile_ctx.cxx_toolchain_info.split_debug_mode or SplitDebugMode("none")
    has_split_debug = split_debug_mode != SplitDebugMode("none")

    strip_dwo_members = _strip_dwo_members_args(
        compile_ctx = compile_ctx,
        emit = emit,
        crate_type = params.crate_type,
        output = emit_op.output,
        has_split_debug = has_split_debug,
    )

    import_library = None
    pdb_artifact = None
    dwp_inputs = []
    link_extraction = None
    if requires_linking:
        if params.crate_type in [CrateType("cdylib"), CrateType("dylib")]:
            linker_info = compile_ctx.cxx_toolchain_info.linker_info
            shlib_name = compile_ctx.soname
            rustc_cmd.add(
                cmd_args(
                    get_shared_library_name_linker_flags(linker_info.type, shlib_name),
                    format = "-Clink-arg={}",
                )
            )
            (import_library, import_library_args) = get_import_library(
                ctx = ctx,
                linker_type = compile_ctx.cxx_toolchain_info.linker_info.type,
                output_short_path = emit_op.output.short_path,
            )
        else:
            import_library_args = []

        subdir = common_args.subdir
        tempfile = common_args.tempfile

        inherited_link_args = precomputed_inherited_link_args
        if inherited_link_args == None:
            inherited_link_args = get_inherited_link_args(
                ctx,
                compile_ctx,
                params.dep_link_strategy,
                None,  # link group info
                transformation_spec_context,
            )

        link_args_output = make_link_args(
            ctx,
            ctx.actions,
            compile_ctx.cxx_toolchain_info,
            [
                LinkArgs(flags = extra_link_args),
                inherited_link_args,
                LinkArgs(flags = import_library_args),
            ],
            output_short_path = emit_op.output.short_path,
            has_content_based_path = emit_cbp,
        )

        separate_debug_info_args = cmd_args()
        if has_split_debug:
            inherited_debug_info = inherited_external_debug_info(
                ctx = ctx,
                dep_ctx = compile_ctx.dep_ctx,
                dep_link_strategy = params.dep_link_strategy,
            )
            external_debug_infos = project_artifacts(ctx.actions, inherited_debug_info)
            dwp_inputs.extend(external_debug_infos)

            # Pass to the link wrapper the paths to the .dwo/.o files to rewrite, if we are
            # using split debug with content-based paths.
            if (
                compile_ctx.cxx_toolchain_info.cxx_compiler_info.supports_content_based_paths
                and
                # Darwin does not embed paths in object files themselves, but rather
                # the linker writes those paths based on the location of object files passed
                # to the link.
                compile_ctx.cxx_toolchain_info.linker_info.type != LinkerType("darwin")
            ):
                # Note: Unlike in C++, Rust does binary linking and object code generation in the same
                # action. As a result, the .dwo files generated by that action are missing from the
                # inputs here and so do not participate in the re-writing. Right now that doesn't matter
                # anyway because Rust doesn't have content addressed artifacts. In the future that may
                # be a source of bugs though.
                separate_debug_info_path_file, _ = ctx.actions.write(
                    "{}/__{}_dwo_paths.txt".format(subdir, tempfile),
                    external_debug_infos,
                    allow_args = True,
                    has_content_based_path = False,
                )
                separate_debug_info_args = cmd_args(
                    "--rewrite-content-based-dwo-paths",
                    separate_debug_info_path_file,
                    "--content-based-dwo-suffix",
                    ".dwo" if split_debug_mode == SplitDebugMode("split") else ".o",
                )

        linker_argsfile, _ = ctx.actions.write(
            "{}/__{}_linker_args.txt".format(subdir, tempfile),
            cmd_args(link_args_output.link_args, separate_debug_info_args),
            allow_args = True,
            has_content_based_path = False,
        )
        linker_argsfile = cmd_args(
            linker_argsfile,
            hidden = [link_args_output.hidden, separate_debug_info_args],
        )

        pdb_artifact = link_args_output.pdb_artifact
        dwp_inputs.append(link_args_output.link_args)

        rustc_cmd.add(cmd_args(linker_argsfile, format = "-Clink-arg=@{}"))
        rustc_cmd.add(cmd_args(compile_ctx.linker_with_pre_args, format = "-Clinker={}"))
    elif extracts_objects:
        # rustc only compiles; the caller links the extracted objects through
        # cxx (see `rust_link_binary`), and the link args are constructed
        # there. rustc never sees them.
        link_extraction = _setup_link_extraction(
            ctx,
            compile_ctx,
            common_args.subdir,
            emit_cbp,
            # FIXME(JakobDegen): Better explain why this is needed
            archive_objects = _dist_thinlto_enabled(ctx, compile_ctx),
        )
        rustc_cmd.add(cmd_args(link_extraction.linker_wrapper, format = "-Clinker={}"))

    if toolchain_info.rust_target_path != None:
        emit_op.env["RUST_TARGET_PATH"] = toolchain_info.rust_target_path[DefaultInfo].default_outputs[0]

    invoke = _rustc_invoke(
        ctx = ctx,
        compile_ctx = compile_ctx,
        common_args = common_args,
        prefix = "{}/{}".format(common_args.subdir, common_args.tempfile),
        rustc_cmd = cmd_args(
            rustc_bin,
            "--rustc-action-separator",
            rustc_cmd,
            emit_op.args,
        ),
        required_outputs = [emit_op.output] if emit_op.output else [],
        is_clippy = emit.value == "clippy",
        infallible_diagnostics = infallible_diagnostics,
        allow_cache_upload = allow_cache_upload and emit != Emit("clippy"),
        crate_map = common_args.crate_map,
        env = emit_op.env,
        incremental_enabled = incremental_enabled,
        profile_mode = profile_mode,
        strip_dwo_members = strip_dwo_members,
    )

    if (emit == Emit("link") or emit == Emit("rlib")) and has_split_debug:
        dwo_output_directory = emit_op.extra_out
        dwp_inputs.append(dwo_output_directory)
    else:
        dwo_output_directory = None

    # When profile_mode is remarks, the remarks are included in the diagnostic stream
    # (same as diag_txt/diag_json), not a separate artifact
    remarks_txt = invoke.diag_txt if profile_mode == ProfileMode("remarks") else None
    remarks_json = invoke.diag_json if profile_mode == ProfileMode("remarks") else None

    if extracts_objects:
        product = link_extraction
    else:
        if infallible_diagnostics and emit != Emit("clippy"):
            # This is only needed when this action's output is being used as an
            # input, so we only need standard diagnostics (clippy is always
            # asked for explicitly).
            filtered_output = failure_filter(
                ctx = ctx,
                compile_ctx = compile_ctx,
                predeclared_output = predeclared_output,
                build_status = invoke.build_status,
                required = emit_op.output,
                stderr = invoke.diag_txt,
                identifier = invoke.identifier,
            )
        else:
            filtered_output = emit_op.output

        singleton_tset = ctx.actions.tset(
            TransitiveDeps,
            value = RustArtifact(
                artifact = filtered_output,
                crate = attr_crate(ctx),
            ),
        )

        # FIXME(JakobDegen): What's going on with stripped objects in binaries? What is this what cxx does?
        if emit in [Emit("rlib"), Emit("link")]:
            stripped_output = strip_debug_info(
                ctx.actions,
                paths.join(
                    common_args.subdir,
                    "stripped",
                    output_filename(
                        compile_ctx,
                        attr_simple_crate_for_filenames(ctx),
                        Emit("link"),
                        params,
                    ),
                ),
                filtered_output,
                compile_ctx.cxx_toolchain_info,
                has_content_based_path = getattr(ctx.attrs, "use_content_based_paths", False),
            )
        else:
            stripped_output = None

        if requires_linking:
            if dwp_available(compile_ctx.cxx_toolchain_info):
                dwp_output = dwp(
                    ctx,
                    compile_ctx.cxx_toolchain_info,
                    emit_op.output,
                    identifier = "{}/__{}_{}_dwp".format(common_args.subdir, common_args.tempfile, emit.value),
                    category_suffix = "rust",
                    # TODO(T110378142): Ideally, referenced objects are a list of
                    # artifacts, but currently we don't track them properly.  So, we
                    # just pass in the full link line and extract all inputs from that,
                    # which is a bit of an overspecification.
                    referenced_objects = dwp_inputs,
                )
            else:
                dwp_output = None
            product = RustcLinkedOutput(
                output = filtered_output,
                singleton_tset = singleton_tset,
                stripped_output = stripped_output,
                import_library = import_library,
                pdb = pdb_artifact,
                dwp_output = dwp_output,
            )
        else:
            product = RustcUnlinkedOutput(
                output = filtered_output,
                singleton_tset = singleton_tset,
                stripped_output = stripped_output,
            )

    return RustcOutput(
        compile_output = RustcCompileOutput(
            diag_txt = invoke.diag_txt,
            diag_json = invoke.diag_json,
            dwo_output_directory = dwo_output_directory,
            profile_output = emit_op.profile_out,
            remarks_txt = remarks_txt,
            remarks_json = remarks_json,
        ),
        product = product,
    )

# --extern <crate>=<path> for direct dependencies
# -Ldependency=<dir> for transitive dependencies
# For native dependencies, we use -Clink-arg=@argsfile
#
# Second element of returned tuple is an @argsfile containing the -Ldependency=<dir> for transitive dependencies.
# It is separate from the first element because some commands (e.g., rustc) do NOT support nested @argsfiles.
#
# Third element of returned tuple is a mapping from crate names back to target
# label, needed for applying autofixes for rustc's unused_crate_dependencies
# lint by tracing Rust crate names in the compiler diagnostic back to which
# dependency entry in the BUCK file needs to be removed.
#
# The `compile_ctx` may be omitted if there are no dependencies with dynamic
# crate names.
#
# cwd: Optional directory the @argsfiles contents will be relative to (e.g., relative -Ldependency paths for rustc).
def dependency_args(
    ctx: AnalysisContext,
    internal_tools_info: RustInternalToolsInfo,
    transitive_dependency_dirs: set[Artifact],
    toolchain_info: RustToolchainInfo,
    deps: list[RustDependency],
    subdir: str,
    dep_link_strategy: LinkStrategy,
    dep_metadata_kind: MetadataKind,
    is_rustdoc_test: bool,
    cwd: Artifact | None = None,
) -> (cmd_args, cmd_args, list[(CrateName, Label)]):
    args = cmd_args()
    transitive_deps = []
    crate_targets = []
    available_proc_macros = get_available_proc_macros(ctx)
    for dep in deps:
        if dep.name:
            crate = CrateName(
                simple = normalize_crate(dep.name),
                dynamic = None,
            )
        else:
            crate = dep.info.crate

        if isinstance(dep.info, RustProcMacroInfo):
            artifact = dep.info.dylib
            singleton_tset = dep.info.singleton_tset
            transitive_artifacts = None
        else:
            strategy = strategy_info(toolchain_info, dep.info, dep_link_strategy)

            # A metadata kind is absent when its artifact wasn't built. Only the
            # `miri` kind can be absent (toolchains without a Miri driver publish
            # none); fail loudly with the owner instead of emitting a dangling
            # `--extern` or a later compiler error. (Direct proc-macro deps take
            # the `dylib` branch above, so they never reach this lookup.)
            artifact = strategy.outputs.get(dep_metadata_kind)
            if artifact == None:
                fail(
                    "dependency `{}` was analyzed without a `{}` artifact (toolchain `{}`): `miri`-kind compiles need every dependency built by a toolchain with `miri_driver`".format(
                        dep.label, dep_metadata_kind.value, toolchain_info.rustc_target_triple or "unknown"
                    )
                )
            singleton_tset = strategy.singleton_tset[dep_metadata_kind]
            transitive_artifacts = strategy.transitive_deps[dep_metadata_kind]

            for label in strategy.transitive_proc_macro_deps:
                transitive_deps.append(available_proc_macros[label][RustProcMacroInfo].singleton_tset)

        args.add(extern_arg(dep.flags, crate, artifact))
        crate_targets.append((crate, dep.label))

        # Because deps of this *target* can also be transitive deps of this
        # compiler invocation, pass the artifact (under its original crate name)
        # through `-L` unconditionally for doc tests.
        if is_rustdoc_test:
            transitive_deps.append(singleton_tset)

        # Unwanted transitive_deps have already been excluded
        if transitive_artifacts != None:
            transitive_deps.append(transitive_artifacts)

    prefix = "{}-deps{}".format(subdir, dep_metadata_kind.value)
    transitive_deps = ctx.actions.tset(TransitiveDeps, children = transitive_deps)
    argsfile = symlinked_dirs(ctx, internal_tools_info, transitive_dependency_dirs, prefix, transitive_deps, cwd)

    return (args, argsfile, crate_targets)

def symlinked_dirs(
    ctx: AnalysisContext,
    internal_tools_info: RustInternalToolsInfo,
    transitive_dependency_dirs: set,
    prefix: str,
    transitive_deps: TransitiveDeps,
    cwd: Artifact | None,
) -> cmd_args:
    name = "{}-symlinked_dirs".format(prefix)

    transitive_dependency_dir = ctx.actions.declare_output(name, dir = True, has_content_based_path = False)

    artifacts = transitive_deps.project_as_json("artifacts")

    # Pass the list of rlibs to transitive_dependency_symlinks.py through a file
    # because there can be a lot of them. This avoids running out of command
    # line length, particularly on Windows.
    artifacts_json = ctx.actions.write_json(
        "{}-symlinked_dirs.json".format(prefix),
        artifacts,
        pretty = True,
        has_content_based_path = getattr(ctx.attrs, "use_content_based_paths", False),
    )

    arguments = [
        internal_tools_info.transitive_dependency_symlinks_tool,
        cmd_args(ctx.label.name, format = "--name={}"),
        cmd_args(transitive_dependency_dir.as_output(), format = "--out-dir={}"),
        cmd_args(
            artifacts_json,
            format = "--artifacts={}",
            # Don't take a dependency on all the artifacts in here, just the dynamic names; the
            # rmetas/rlibs we only want to create symlinks to, so there's no need for them to
            # actually be available
            hidden = transitive_deps.project_as_args("dynamic_name_args"),
        ),
    ]

    if cwd:
        arguments.append(
            cmd_args(
                transitive_dependency_dir.as_output(),
                format = "--out-dir-relative-to-cwd={}",
                relative_to = cwd,
            )
        )

    ctx.actions.run(
        arguments,
        category = "deps",
        identifier = str(len(transitive_dependency_dirs)),
    )

    transitive_dependency_dirs.add(transitive_dependency_dir)

    return cmd_args(
        # Reference the directory Artifact (not the dirs file), so all of its children are included.
        transitive_dependency_dir,
        format = "@{}/dirs",
        hidden = transitive_deps.project_as_args("artifacts_args"),
    )

def _lintify(flag: str, clippy: bool, lints: list[str | ResolvedStringWithMacros]) -> cmd_args:
    return cmd_args(
        [lint for lint in lints if clippy or not str(lint).startswith('"clippy::')],
        format = "-{}{{}}".format(flag),
    )

def _lint_flags(compile_ctx: CompileContext, infallible_diagnostics: bool, is_clippy: bool) -> cmd_args:
    toolchain_info = compile_ctx.toolchain_info

    return cmd_args(
        _lintify("A", is_clippy, toolchain_info.allow_lints),
        _lintify("D", is_clippy, toolchain_info.deny_lints),
        _lintify("D" if infallible_diagnostics else "W", is_clippy, toolchain_info.deny_on_check_lints),
        _lintify("W", is_clippy, toolchain_info.warn_lints),
    )

def _check_restricted_rustc_flags(flags: list[str | ResolvedStringWithMacros | Artifact], toolchain_info: RustToolchainInfo):
    if not toolchain_info.restricted_rustc_flags:
        return
    for flag in flags:
        if isinstance(flag, str):
            for restricted in toolchain_info.restricted_rustc_flags:
                if restricted in flag:
                    fail(
                        "rustc flag `{}` is not allowed on individual targets ".format(flag)
                        + "(matched restricted pattern `{}`). ".format(restricted)
                        + "This flag must be set in the Rust toolchain, not on individual targets, "
                        + "to ensure consistent compilation across the build graph. "
                        + "If you have a legitimate need, set `uses_restricted_rustc_flags = True` on your target.",
                    )

def _rustc_flags(flags: list[str | ResolvedStringWithMacros | Artifact], toolchain_info: RustToolchainInfo) -> list[str | ResolvedStringWithMacros | Artifact]:
    # Rustc's "-g" flag is documented as being exactly equivalent to
    # "-Cdebuginfo=2". Rustdoc supports the latter, it just doesn't have the
    # "-g" shorthand for it.
    for i, flag in enumerate(flags):
        flag = str(flag).strip('"')
        if flag == "-g":
            flags[i] = "-Cdebuginfo=2"
        if toolchain_info.advanced_unstable_linking:
            # The extraction drops rustc's linker argv apart from its synthesized
            # inputs, so flags injected via `-Clink-arg` would be silently lost.
            # `uses_restricted_rustc_flags` does not exempt this: unlike the flags
            # that attribute normally guards, these would not merely be
            # unsupported, they would silently not happen.
            if "-Clink-arg" in flag or (flag == "-C" and i + 1 < len(flags) and str(flags[i + 1]).strip('"').startswith("link-arg")):
                fail("flags passed via `-Clink-arg` are dropped when linking through cxx; use the target's or toolchain's `linker_flags` instead")

            # Additional flags that rustc only forwards to the linker
            if "-Cstrip" in flag or (flag == "-C" and i + 1 < len(flags) and str(flags[i + 1]).strip('"').startswith("strip")):
                fail("`-Cstrip` has no effect when linking through cxx; use the `symbols` strip_mode, or `-Wl,--strip-all`/`-Wl,-S` in `linker_flags`, instead")
            if "-Crpath" in flag or (flag == "-C" and i + 1 < len(flags) and str(flags[i + 1]).strip('"').startswith("rpath")):
                fail("`-Crpath` has no effect when linking through cxx; pass `-Wl,-rpath,...` in `linker_flags` instead")
            if "-Clink-dead-code" in flag or (flag == "-C" and i + 1 < len(flags) and str(flags[i + 1]).strip('"').startswith("link-dead-code")):
                fail("`-Clink-dead-code` has no effect when linking through cxx; pass `-Wl,--no-gc-sections` in `linker_flags` instead")

        if toolchain_info.explicit_sysroot_deps:
            # `-Clink-self-contained` selects linker inputs bundled in the
            # sysroot, and explicit sysroot deps point rustc at an empty one,
            # so everything it can enable is a dangling reference. Only the
            # subtractive forms make sense (the toolchain itself uses them to
            # keep rustc from consulting the empty sysroot).
            value = None
            if flag.startswith("-Clink-self-contained"):
                value = flag.removeprefix("-Clink-self-contained").removeprefix("=")
            elif flag == "-C" and i + 1 < len(flags):
                next_flag = str(flags[i + 1]).strip('"')
                if next_flag.startswith("link-self-contained"):
                    value = next_flag.removeprefix("link-self-contained").removeprefix("=")
            if value != None:
                for part in value.split(","):
                    if part == "" or part in ("y", "yes", "on", "true") or part.startswith("+"):
                        fail(
                            "`-Clink-self-contained` cannot enable bundled linker inputs: explicit sysroot deps leave the sysroot empty, so there is nothing there to link"
                        )

    return flags

# Differently parameterized build outputs need to be assigned nonoverlapping
# output paths. For example the pic and non-pic rlib cannot both be written to
# libfoo.rlib. We place artifacts into a unique subdirectory for each
# permutation of build parameters.
#
# Keep this short or it exacerbates filepath length limits on Windows.
#
# Common examples:
#     rlib pic static_pic metadata-fast diag => "LPPMD"
#     bin pic shared link => "XPHL"
def _abbreviated_subdir(
    crate_type: CrateType,
    reloc_model: RelocModel,
    dep_link_strategy: LinkStrategy,
    emit: Emit,
    is_rustdoc_test: bool,
    is_miri_run: bool,
    infallible_diagnostics: bool,
    incremental_enabled: bool,
    profile_mode: ProfileMode | None,
) -> str:
    crate_type = {
        CrateType("bin"): "X",  # mnemonic: "eXecutable"
        CrateType("rlib"): "L",  # "Library"
        CrateType("dylib"): "D",
        CrateType("proc-macro"): "M",  # "Macro"
        CrateType("cdylib"): "C",
        CrateType("staticlib"): "S",
    }[crate_type]

    reloc_model = {
        RelocModel("static"): "S",
        RelocModel("pic"): "P",
        RelocModel("pie"): "I",
        RelocModel("dynamic-no-pic"): "N",
        RelocModel("ropi"): "O",
        RelocModel("rwpi"): "W",
        RelocModel("ropi-rwpi"): "R",
        RelocModel("default"): "D",
    }[reloc_model]

    dep_link_strategy = {
        LinkStrategy("static"): "T",
        LinkStrategy("static_pic"): "P",
        LinkStrategy("shared"): "H",
    }[dep_link_strategy]

    emit = {
        Emit("asm"): "s",
        Emit("llvm-bc"): "b",
        Emit("llvm-ir"): "i",
        Emit("llvm-ir-noopt"): "n",
        Emit("obj"): "o",
        Emit("link"): "L",
        Emit("rlib"): "R",
        Emit("dep-info"): "d",
        Emit("mir"): "m",
        Emit("expand"): "e",
        Emit("clippy"): "c",
        Emit("miri"): "U",  # "Undefined behavior"
        Emit("metadata-full"): "F",  # "Full metadata"
        Emit("metadata-fast"): "M",  # "Metadata"
    }[emit]

    profile_mode = {
        None: "",
        ProfileMode("llvm-time-trace"): "L",
        ProfileMode("self-profile"): "P",
        ProfileMode("remarks"): "R",
    }[profile_mode]

    return (
        crate_type
        + reloc_model
        + dep_link_strategy
        + emit
        + ("T" if is_rustdoc_test else "")
        + ("R" if is_miri_run else "")
        + ("D" if infallible_diagnostics else "")
        + ("I" if incremental_enabled else "")
        + profile_mode
    )

# Compute which are common to both rustc and rustdoc
def _compute_common_args(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    dep_ctx: DepCollectionContext,
    emit: Emit,
    params: BuildParams,
    default_roots: list[str],
    infallible_diagnostics: bool,
    incremental_enabled: bool,
    is_rustdoc_test: bool,
    profile_mode: ProfileMode | None,
    # The args feed `miri_command`, run at test time through `rustc_action.py`.
    is_miri_run: bool = False,
) -> CommonArgsInfo:
    crate_type = params.crate_type

    args_key = (crate_type, emit, params.dep_link_strategy, is_rustdoc_test, is_miri_run, infallible_diagnostics, incremental_enabled, profile_mode)
    if args_key in compile_ctx.common_args:
        return compile_ctx.common_args[args_key]

    subdir = _abbreviated_subdir(
        crate_type = crate_type,
        reloc_model = params.reloc_model,
        dep_link_strategy = params.dep_link_strategy,
        emit = emit,
        is_rustdoc_test = is_rustdoc_test,
        is_miri_run = is_miri_run,
        infallible_diagnostics = infallible_diagnostics,
        incremental_enabled = incremental_enabled,
        profile_mode = profile_mode,
    )

    # Included in tempfiles
    tempfile = "{}-{}".format(attr_simple_crate_for_filenames(ctx), emit.value)

    root = crate_root(ctx, default_roots)
    if compile_ctx.exec_is_windows:
        root = root.replace("/", "\\")

    dep_metadata_kind = dep_metadata_of_emit(emit)

    if dep_metadata_kind == MetadataKind("link") and (is_rustdoc_test or (compile_ctx.dep_ctx.advanced_unstable_linking and crate_type_linked(crate_type))):
        # AUL rlibs compile against full metadata, so Rustdoc tests and
        # rustc-linked outputs need the same full metadata deps to avoid crate
        # hash mismatches. The linker receives the real rlibs through inherited
        # link args.
        dep_metadata_kind = MetadataKind("full")

    dependency_args_key = (crate_type, params.dep_link_strategy, dep_metadata_kind, is_rustdoc_test)
    cached_dependency_args = compile_ctx.dependency_args.get(dependency_args_key)
    if cached_dependency_args == None:
        dep_args, dep_argsfiles, crate_map = dependency_args(
            ctx = ctx,
            internal_tools_info = compile_ctx.internal_tools_info,
            transitive_dependency_dirs = compile_ctx.transitive_dependency_dirs,
            toolchain_info = compile_ctx.toolchain_info,
            deps = resolve_rust_deps(ctx, dep_ctx),
            subdir = subdir,
            dep_link_strategy = params.dep_link_strategy,
            dep_metadata_kind = dep_metadata_kind,
            is_rustdoc_test = is_rustdoc_test,
        )

        dep_args.add(
            cmd_args(
                hidden = compile_ctx.transitive_srcs.project_as_args("artifacts") if compile_ctx else [],
            )
        )

        # Add dep_argsfiles to dep_args because rustc_action supports nested @argfiles.
        dep_args.add(dep_argsfiles)

        if crate_type == CrateType("proc-macro"):
            dep_args.add("--extern=proc_macro")

        cached_dependency_args = DependencyArgsInfo(
            args = dep_args,
            crate_map = crate_map,
        )
        compile_ctx.dependency_args[dependency_args_key] = cached_dependency_args

    dep_args = cached_dependency_args.args
    crate_map = cached_dependency_args.crate_map

    toolchain_info = compile_ctx.toolchain_info
    edition = ctx.attrs.edition or toolchain_info.default_edition or fail("missing 'edition' attribute, and there is no 'default_edition' set by the toolchain")

    crate = attr_crate(ctx)
    if crate.dynamic:
        crate_name_arg = cmd_args("--crate-name", cmd_args("@", crate.dynamic, delimiter = ""))
    else:
        crate_name_arg = cmd_args("--crate-name=", crate.simple, delimiter = "")

    # The `-Cprefer-dynamic` flag controls rustc's choice of artifacts for
    # transitive dependencies, both for loading metadata and linking them.
    # Direct dependencies are given to rustc one-by-one using `--extern` with a
    # path to a specific artifact, so there is never ambiguity what artifact to
    # use for a direct dependency. But transitive dependencies are passed in
    # bulk via zero or more `-Ldependency` flags, which are directories
    # containing artifacts. Within those directories, information about a
    # specific crate might be available from more than one artifact, such as a
    # dylib and rlib for the same crate.
    #
    # With `-Cprefer-dynamic=no` (the default), when a transitive dependency
    # exists as both rlib and dylib, metadata is loaded from the rlib. If some
    # dependencies are available in dylib but not rlib, the dylib is used for
    # those. With `-Cprefer-dynamic=yes`, when a transitive dependency exists as
    # both rlib and dylib, instead the dylib is used.
    #
    # The ambiguity over whether to use rlib or dylib for a particular
    # transitive dependency only occurs if the rlib and dylib both describe the
    # same crate i.e. contain the same crate hash.
    #
    # Buck-built libraries never produce an rlib and dylib containing the same
    # crate hash, since that only occurs when outputting multiple crate types
    # through a single rustc invocation: `--crate-type=rlib --crate-type=dylib`.
    # In Buck, different crate types are built by different rustc invocations.
    # But Cargo does invoke rustc with multiple crate types when you write
    # `[lib] crate-type = ["rlib", "dylib"]` in Cargo.toml, and in fact the
    # standard libraries built by x.py and distributed by Rustup are built this
    # way.
    if toolchain_info.explicit_sysroot_deps:
        # Standard libraries are being passed explicitly, and Buck-built
        # dependencies never collide on crate hash, so `-Cprefer-dynamic` cannot
        # make a difference.
        prefer_dynamic_flags = []
    elif crate_type == CrateType("dylib") and toolchain_info.advanced_unstable_linking:
        # Use standard library dylibs from the implicit sysroot.
        prefer_dynamic_flags = ["-Cprefer-dynamic=yes"]
    else:
        # Use standard library rlibs from the implicit sysroot.
        prefer_dynamic_flags = ["-Cprefer-dynamic=no"]  # (the default)

    split_debuginfo_flags = {
        # Rustc's default behavior: debug info is put into every rlib and
        # staticlib, then copied into the executables and shared libraries by
        # the linker. This corresponds to `-gno-split-dwarf` in Clang.
        SplitDebugMode("none"): [],
        # Split DWARF: debug info is placed into *.dwo files in the directory
        # specified by `--out-dir`. In Buck, this directory is usually called
        # "extras" that is a sibling of the main output artifact (rlib,
        # staticlib, executable, or shared library).
        #
        # Rustc produces one *.dwo per LLVM codegen unit, meaning potentially
        # multiple per crate. The only debug info included into the main output
        # artifact is the list of the associated *.dwo filenames in which the
        # real debug info is provided.
        #
        # For each binary target, we have a separate step which involves
        # `llvm-dwp` to combine all the *.dwo files from the dependency graph
        # into one *.dwp. This is handled as a separate Buck action from the
        # compiler/linker invocation responsible for linking the executable or
        # shared library artifact.
        #
        # Rust's `-Csplit-debuginfo=unpacked` corresponds to `-gsplit-dwarf=split`
        # in Clang, which behaves as just described.
        #
        # There is a second Clang debug mode, `-gsplit-dwarf=single`, that we
        # also implement using `-Csplit-debuginfo=unpacked` in Rust. In Clang,
        # "single" means include debug info into object files conceptually like
        # `-gno-split-dwarf`, but do _not_ copy it at link time into executables
        # and shared libraries. Similar to "split", there is an `llvm-dwp` step,
        # separate from linking, to combine debug info from the dependency graph
        # into one *.dwp output. Rustc has an option `-Csplit-debuginfo=packed`
        # which works this way, putting split debug info into rlibs for
        # libraries but keeping it separate for binaries. We have not been able
        # to use `-Csplit-debuginfo=packed` because it runs into an error "unit
        # referenced by executable was not found" when dealing with chains of
        # dependencies from Rust -> C++ -> Rust (T147665047).
        SplitDebugMode("single"): ["-Csplit-debuginfo=unpacked"],
        SplitDebugMode("split"): ["-Csplit-debuginfo=unpacked"],
    }[compile_ctx.cxx_toolchain_info.split_debug_mode or SplitDebugMode("none")]

    if not getattr(ctx.attrs, "uses_restricted_rustc_flags", False):
        _check_restricted_rustc_flags(ctx.attrs.rustc_flags, toolchain_info)

    args = cmd_args(
        cmd_args(compile_ctx.symlinked_srcs, compile_ctx.path_sep, root, delimiter = ""),
        crate_name_arg,
        "--crate-type={}".format(crate_type.value),
        "-Crelocation-model={}".format(params.reloc_model.value),
        "--edition={}".format(edition),
        "-Cmetadata={}".format(
            _metadata(
                compile_ctx,
                ctx.label,
                is_rustdoc_test,
                _host_agnostic_proc_macro(crate_type, emit, is_rustdoc_test, incremental_enabled),
            )[0]
        ),
        # JSON diagnostics are for `rustc_action.py`; commands run at test time
        # print rendered text directly.
        ["--error-format=json", "--json=diagnostic-rendered-ansi"] if not (is_rustdoc_test or is_miri_run) else [],
        prefer_dynamic_flags,
        ["--target={}".format(toolchain_info.rustc_target_triple)] if toolchain_info.rustc_target_triple else [],
        split_debuginfo_flags,
        compile_ctx.sysroot_args,
        # `-Zpanic-abort-tests` makes the test harness work despite the abort
        # runtime. Without nightly features it can't be passed; test targets
        # will be rejected by rustc.
        (["-Cpanic=abort"] + (["-Zpanic-abort-tests=yes"] if toolchain_info.nightly_features else []))
        if toolchain_info.panic_runtime == PanicRuntime("abort")
        else [],
        ["-Zsanitizer={}".format(toolchain_info.sanitizer.value)] if toolchain_info.sanitizer else [],
        ["-Cprofile-generate={}".format(toolchain_info.pgo_generate_dir), "-Zno-profiler-runtime"]
        if toolchain_info.pgo_generate_dir and toolchain_info.explicit_sysroot_deps and toolchain_info.explicit_sysroot_deps.core
        else [],
        [cmd_args(toolchain_info.pgo_profile, format = "-Cprofile-use={}")]
        if toolchain_info.pgo_profile and toolchain_info.explicit_sysroot_deps and toolchain_info.explicit_sysroot_deps.core
        else [],
        _rustc_flags(toolchain_info.rustc_flags, toolchain_info),
        _rustc_flags(toolchain_info.rustc_proc_macro_flags, toolchain_info)
        if _host_agnostic_proc_macro(crate_type, emit, is_rustdoc_test, incremental_enabled)
        else [],
        # `rustc_check_flags` is specifically interpreted as flags that are used
        # only on the metadata-fast graph.
        _rustc_flags(toolchain_info.rustc_check_flags, toolchain_info) if dep_metadata_kind == MetadataKind("fast") else [],
        _rustc_flags(toolchain_info.rustc_coverage_flags, toolchain_info) if ctx.attrs.coverage else [],
        _rustc_flags(ctx.attrs.rustc_flags, toolchain_info),
        _rustc_flags(toolchain_info.extra_rustc_flags, toolchain_info),
        cmd_args(ctx.attrs.features, format = '--cfg=feature="{}"'),
        dep_args,
    )

    common_args = CommonArgsInfo(
        args = args,
        subdir = subdir,
        tempfile = tempfile,
        crate_type = crate_type,
        params = params,
        emit = emit,
        crate_map = crate_map,
    )

    compile_ctx.common_args[args_key] = common_args
    return common_args

def rust_compile_invalidation_inputs(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    emit: Emit,
    params: BuildParams,
    default_roots: list[str],
    incremental_enabled: bool,
) -> cmd_args:
    return _compute_common_args(
        ctx = ctx,
        compile_ctx = compile_ctx,
        dep_ctx = compile_ctx.dep_ctx,
        emit = emit,
        params = params,
        default_roots = default_roots,
        infallible_diagnostics = False,
        incremental_enabled = incremental_enabled,
        is_rustdoc_test = False,
        profile_mode = None,
    ).args

def _host_agnostic_proc_macro(crate_type: CrateType, emit: Emit, is_rustdoc_test: bool, incremental_enabled: bool) -> bool:
    return crate_type == CrateType("proc-macro") and emit != Emit("clippy") and not is_rustdoc_test and not incremental_enabled

# Returns the full label and its hash. The full label is used for `-Cmetadata`
# which provided the primary disambiguator for two otherwise identically named
# crates. The hash is added to the filename to give them a lower likelihood of
# duplicate names, but it doesn't matter if they collide.
def _metadata(
    compile_ctx: CompileContext,
    label: Label,
    is_rustdoc_test: bool,
    host_agnostic_proc_macro: bool,
) -> (str, str):
    raw_target = str(label.raw_target())
    configuration_hash = (
        compile_ctx.toolchain_info.proc_macro_configuration_hash if host_agnostic_proc_macro else compile_ctx.toolchain_info.configuration_hash
    ) or label.configured_target().config().hash
    metadata = "{}#{}".format(raw_target, configuration_hash)

    if is_rustdoc_test:
        metadata = "doctest/" + metadata

    int_hash = hash(metadata)
    if int_hash < 0:
        int_hash = -int_hash

    hex_hash = "%x" % int_hash
    hex_hash = "0" * (8 - len(hex_hash)) + hex_hash

    return (metadata, hex_hash)

def crate_root(ctx: AnalysisContext, default_roots: list[str]) -> str:
    if ctx.attrs.crate_root:
        return ctx.attrs.crate_root

    srcs = [s.short_path for s in ctx.attrs.srcs] + ctx.attrs.mapped_srcs.values()

    candidates = set()
    if getattr(ctx.attrs, "crate_dynamic", None):
        crate_with_suffix = None
    else:
        crate_with_suffix = attr_crate(ctx).simple + ".rs"
    for src in srcs:
        filename = src.split("/")[-1]
        if filename in default_roots or filename == crate_with_suffix:
            candidates.add(src)

    if len(candidates) == 1:
        return candidates.pop()

    fail(
        "Could not infer crate_root."
        + "\nMake sure you have one of {} in your `srcs` attribute.".format(default_roots)
        + "\nOr add 'crate_root = \"src/example.rs\"' to your attributes to disambiguate. candidates={}".format(candidates)
    )

def _explain(
    crate_type: CrateType,
    link_strategy: LinkStrategy,
    emit: Emit,
    infallible_diagnostics: bool,
    profile_mode: ProfileMode | None,
    advanced_unstable_linking: bool,
) -> str:
    base = None
    if emit == Emit("metadata-full"):
        link_strategy_suffix = {
            LinkStrategy("static"): " [static]",
            LinkStrategy("static_pic"): " [pic]",
            LinkStrategy("shared"): " [shared]",
        }[link_strategy]
        base = "metadata" + link_strategy_suffix

    if emit == Emit("metadata-fast"):
        base = "diag" if infallible_diagnostics else "check"

    if emit == Emit("rlib"):
        if crate_type == CrateType("rlib"):
            base = "rlib-no-meta" if advanced_unstable_linking else "rlib"
        else:
            base = "rlib-from-link"
        base += {
            LinkStrategy("static"): "",
            LinkStrategy("static_pic"): " [pic]",
            LinkStrategy("shared"): " [shared]",
        }[link_strategy]

    if emit == Emit("miri"):
        base = (
            "miri"
            + {
                LinkStrategy("static"): "",
                LinkStrategy("static_pic"): " [pic]",
                LinkStrategy("shared"): " [shared]",
            }[link_strategy]
        )

    if emit == Emit("link"):
        link_strategy_suffix = {
            LinkStrategy("static"): "",
            LinkStrategy("static_pic"): " [pic]",
            LinkStrategy("shared"): " [shared]",
        }[link_strategy]
        if crate_type == CrateType("bin"):
            base = "link" + link_strategy_suffix
        if crate_type == CrateType("rlib"):
            base = "rlib" + link_strategy_suffix
        if crate_type == CrateType("dylib"):
            base = "dylib" + link_strategy_suffix
        if crate_type == CrateType("proc-macro"):
            base = "proc-macro"  # always static_pic
        if crate_type == CrateType("cdylib"):
            base = "cdylib" + link_strategy_suffix
        if crate_type == CrateType("staticlib"):
            base = "staticlib" + link_strategy_suffix

    if emit == Emit("expand"):
        base = "expand"

    for emit_type in ["asm", "llvm-ir", "mir"]:
        if emit == Emit(emit_type):
            link_strategy_suffix = {
                LinkStrategy("static"): " [static]",
                LinkStrategy("static_pic"): " [pic]",
                LinkStrategy("shared"): " [shared]",
            }[link_strategy]
            base = emit_type + link_strategy_suffix

    if emit == Emit("llvm-ir-noopt"):
        base = "llvm-ir-noopt"

    if base == None:
        fail("unrecognized rustc action:", crate_type, link_strategy, emit)

    if profile_mode:
        return "{} [{}]".format(base, profile_mode.value)
    else:
        return base

EmitOperation = record(
    # None exactly for bin-crate `Emit("rlib")`: the caller owns the linked
    # artifact, and rustc produces only the extraction outputs.
    output = field(Artifact | None),
    args = field(cmd_args),
    env = field(dict[str, str]),
    extra_out = field(Artifact | None),
    profile_out = field(Artifact | None),
)

# Take a desired output and work out how to convince rustc to generate it
def _rustc_emit(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    emit: Emit,
    subdir: str,
    params: BuildParams,
    incremental_enabled: bool,
    profile_mode: ProfileMode | None,
    predeclared_output: Artifact | None = None,
) -> EmitOperation:
    simple_crate = attr_simple_crate_for_filenames(ctx)
    crate_type = params.crate_type

    emit_args = cmd_args()
    emit_env = {}
    extra_out = None
    profile_out = None
    use_cbp = getattr(ctx.attrs, "use_content_based_paths", False)

    # When a predeclared_output is used (e.g. from rust_binary), all related
    # outputs must match its content-based path setting (False) so that
    # side-effect outputs like PDB are consistent with the binary.
    emit_cbp = use_cbp if predeclared_output == None else False

    if predeclared_output:
        emit_output = predeclared_output

        # Don't support profiles with predeclared outputs
        crate_name_and_extra_for_profile = None
    else:
        extra_hash = (
            "-"
            + _metadata(
                compile_ctx,
                ctx.label,
                False,
                _host_agnostic_proc_macro(crate_type, emit, False, incremental_enabled),
            )[1]
        )
        emit_args.add("-Cextra-filename={}".format(extra_hash))
        crate_name_and_extra_for_profile = simple_crate + extra_hash

        if emit == Emit("rlib") and params.crate_type == CrateType("bin"):
            # The caller links the extracted objects through cxx and owns the
            # linked artifact; there is no rustc output to declare.
            emit_output = None
        else:
            filename = subdir + "/" + output_filename(compile_ctx, simple_crate, emit, params, extra_hash)
            emit_output = ctx.actions.declare_output(filename, has_content_based_path = emit_cbp)

    if emit == Emit("expand"):
        emit_args.add(
            "-Zunpretty=expanded",
            cmd_args(emit_output.as_output(), format = "-o{}"),
        )
    else:
        if emit == Emit("metadata-full"):
            if crate_type not in (CrateType("rlib"), CrateType("dylib")):
                # Nothing ever needs the metadata from these crate types, so we can
                # fall back to the `metadata-fast` behavior. Normally though, this
                # artifact should be unused and so this shouldn't matter.
                effective_emit = "metadata"
            else:
                # As we're doing a pipelined build, instead of emitting an actual rmeta
                # we emit a "hollow" .rlib - ie, it only contains lib.rmeta and no object
                # code. It should contain full information needed by any dependent
                # crate which is generating code (MIR, etc).
                #
                # IMPORTANT: this flag is the only way that the Emit("metadata") and
                # Emit("link") operations are allowed to diverge without causing them to
                # get different crate hashes.
                if not compile_ctx.toolchain_info.nightly_features:
                    fail("Pipelined builds use the unstable `-Zno-codegen` flag and require a toolchain with `nightly_features = True`")
                emit_args.add("-Zno-codegen")
                effective_emit = "link"
        elif emit == Emit("metadata-fast") or emit == Emit("clippy"):
            effective_emit = "metadata"
        elif emit == Emit("llvm-ir-noopt"):
            effective_emit = "llvm-ir"
            emit_args.add("-Cno-prepopulate-passes")
        elif emit == Emit("rlib"):
            # rlibs are `--emit=link` products of the rlib crate type. For bin
            # crates there is additionally no output to bind: the extraction
            # wrapper set via `-Clinker=` captures the objects instead.
            effective_emit = "link"
        elif emit == Emit("miri"):
            # Interpreted from the MIR in the rmeta; nothing runs natively.
            emit_args.add("-Zno-codegen")
            effective_emit = "link"
        else:
            effective_emit = emit.value

        # Pipelined builds still need metadata in `metadata-full`'s hollow rlib;
        # native-linkable artifacts can omit it.
        if compile_ctx.toolchain_info.advanced_unstable_linking and (
            (emit == Emit("rlib") and crate_type == CrateType("rlib")) or (emit == Emit("link") and crate_type == CrateType("dylib"))
        ):
            emit_args.add("-Zembed-metadata=no")

        if emit_output == None:
            emit_args.add(cmd_args("--emit=", effective_emit, delimiter = ""))
        else:
            emit_args.add(cmd_args("--emit=", effective_emit, "=", emit_output.as_output(), delimiter = ""))

        # Strip file extension from directory name.
        base, _ext = paths.split_extension(output_filename(compile_ctx, simple_crate, emit, params))
        extra_dir = subdir + "/extras/" + base
        extra_out = ctx.actions.declare_output(extra_dir, dir = True, has_content_based_path = False)
        emit_args.add(cmd_args(extra_out.as_output(), format = "--out-dir={}"))

        if incremental_enabled:
            incremental_out = ctx.actions.declare_output("{}/extras/incremental".format(subdir), has_content_based_path = False)
            incremental_cmd = cmd_args(incremental_out.as_output(), format = "-Cincremental={}")
            emit_args.add(incremental_cmd)

        if profile_mode == ProfileMode("llvm-time-trace"):
            emit_args.add("-Zllvm-time-trace=yes")
            profile_out = extra_out.project(crate_name_and_extra_for_profile + ".llvm_timings.json")
        elif profile_mode == ProfileMode("self-profile"):
            self_profile = ctx.actions.declare_output("{}/extra/self-profile".format(subdir), dir = True, has_content_based_path = False)
            emit_args.add("-Zself-profile-events=default,args")
            emit_args.add(cmd_args("-Zself-profile=", self_profile.as_output(), delimiter = ""))
            profile_out = self_profile
        elif profile_mode == ProfileMode("remarks"):
            # Enable LLVM remarks - they appear in the diagnostic stream (stderr)
            # Use the configured filter, or default to "all"
            # Allow comma-separated values for convenience (rustc expects space-separated)
            remarks_filter = (compile_ctx.toolchain_info.remarks or "all").replace(",", " ")
            emit_args.add("-Cremark={}".format(remarks_filter))

    return EmitOperation(
        output = emit_output,
        args = emit_args,
        env = emit_env,
        extra_out = extra_out,
        profile_out = profile_out,
    )

Invoke = record(
    diag_txt = field(Artifact),
    diag_json = field(Artifact),
    build_status = field(Artifact | None),
    identifier = field(str | None),
)

# Under `-Csplit-debuginfo=unpacked` (see `split_debuginfo_flags` in
# `_compute_common_args`), rustc writes each codegen unit's `.dwo` to
# `--out-dir` and also packs a copy of it into the rlib or staticlib, so that a
# downstream `-Csplit-debuginfo=packed` link could build a dwp from the archive
# alone. These rules never link that way: `dwp` reads the `--out-dir` files,
# which flow to it as external debug info, and linkers never pull archive
# members that define no symbols. A toolchain that sets `strip_dwo_from_rlibs`
# has `rustc_action.py` delete those members inside the compile action, so the
# archive is never cached or handed to a consumer with them.
def _strip_dwo_members_args(compile_ctx: CompileContext, emit: Emit, crate_type: CrateType, output: Artifact | None, has_split_debug: bool) -> cmd_args | None:
    if not compile_ctx.toolchain_info.strip_dwo_from_rlibs or not has_split_debug:
        return None

    # Only the `--emit=link` products of the archive crate types carry `.dwo`
    # members. `Emit("rlib")` of an rlib crate is that product; the hollow
    # rlibs of `metadata-full` are built without codegen and have none.
    if crate_type not in [CrateType("rlib"), CrateType("staticlib")]:
        return None
    if emit not in [Emit("link"), Emit("rlib")] or output == None:
        return None

    linker_info = compile_ctx.cxx_toolchain_info.linker_info

    # rustc emits DWARF objects only for targets whose debuginfo is plain DWARF
    # (`Session::target_can_use_split_dwarf`): Apple (dSYM) and Windows (PDB)
    # archives never carry `.dwo` members, so skip the archiver round-trip.
    if linker_info.type in [LinkerType("darwin"), LinkerType("windows")]:
        return None

    # `d` is the member-deletion verb of the GNU, llvm and BSD archivers; the
    # others (`lib.exe`, amdclang) have no equivalent this tool speaks, so
    # toolchains using them keep the members.
    if linker_info.archiver_type not in ["gnu", "llvm", "bsd"]:
        return None

    return cmd_args(
        cmd_args(output.as_output(), format = "--strip-dwo-members={}"),
        cmd_args(linker_info.archiver, format = "--archiver={}"),
        # As in `archive_flags`: GNU-style archivers need `D` to leave
        # timestamps out of the archive they write.
        ["--archiver-deterministic"] if linker_info.type == LinkerType("gnu") else [],
    )

# Invoke rustc and capture outputs
def _rustc_invoke(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    common_args: CommonArgsInfo,
    prefix: str,
    rustc_cmd: cmd_args,
    required_outputs: list[Artifact],
    is_clippy: bool,
    infallible_diagnostics: bool,
    allow_cache_upload: bool,
    incremental_enabled: bool,
    crate_map: list[(CrateName, Label)],
    env: dict[str, str | ResolvedStringWithMacros | Artifact],
    profile_mode: ProfileMode | None,
    strip_dwo_members: cmd_args | None,
) -> Invoke:
    toolchain_info = compile_ctx.toolchain_info

    plain_env, path_env = _base_rustc_env(ctx, compile_ctx)

    more_plain_env, more_path_env = process_env(compile_ctx, env)
    plain_env.update(more_plain_env)
    path_env.update(more_path_env)

    # `nightly_features` grants the rules use of unstable functionality. Both
    # internally and in OSS the compiler is typically a stable-channel build,
    # where that functionality must be unlocked by setting `RUSTC_BOOTSTRAP=1`;
    # on an actual nightly compiler the variable is harmless. This must be
    # uniform across all invocations for a crate: rustc includes the variable
    # in its crate hashes, so differences between e.g. the metadata and link
    # commands cause "found possibly newer version of crate" errors.
    if toolchain_info.nightly_features and "RUSTC_BOOTSTRAP" not in plain_env:
        plain_env["RUSTC_BOOTSTRAP"] = "1"

    # Save diagnostic outputs
    diag = "clippy" if is_clippy else "diag"
    use_cbp = getattr(ctx.attrs, "use_content_based_paths", False)
    diag_json = ctx.actions.declare_output("{}-{}.json".format(prefix, diag), has_content_based_path = use_cbp)
    diag_txt = ctx.actions.declare_output("{}-{}.txt".format(prefix, diag), has_content_based_path = use_cbp)

    compile_cmd = cmd_args(
        cmd_args(diag_json.as_output(), format = "--diag-json={}"),
        cmd_args(diag_txt.as_output(), format = "--diag-txt={}"),
        ["--remap-cwd-prefix=."] if not toolchain_info.nightly_features else [],
        "--buck-target={}".format(ctx.label.raw_target()),
        hidden = [toolchain_info.compiler, compile_ctx.transitive_srcs.project_as_args("artifacts")],
    )

    for k, v in crate_map:
        compile_cmd.add(crate_map_arg(k, v))
    for k, v in plain_env.items():
        compile_cmd.add(cmd_args("--env=", k, "=", v, delimiter = ""))
    for k, v in path_env.items():
        compile_cmd.add(cmd_args("--path-env=", k, "=", v, delimiter = ""))

    build_status = None
    if infallible_diagnostics:
        # Build status for fail filter
        build_status = ctx.actions.declare_output("{}_build_status-{}.json".format(prefix, diag), has_content_based_path = use_cbp)
        compile_cmd.add(cmd_args(build_status.as_output(), format = "--failure-filter={}"))
        for out in required_outputs:
            compile_cmd.add("--required-output", out.short_path, out.as_output())
        if common_args.emit == Emit("expand"):
            compile_cmd.add("--keep-outputs-on-error")

    if strip_dwo_members != None:
        compile_cmd.add(strip_dwo_members)

    compile_cmd.add(rustc_cmd)

    compile_cmd = _long_command(
        ctx = ctx,
        exe = compile_ctx.internal_tools_info.rustc_action,
        args = compile_cmd,
        argfile_name = "{}-{}.args".format(prefix, diag),
        has_content_based_path = use_cbp,
    )

    local_only = incremental_enabled
    prefer_local = False
    weight = 1
    if common_args.crate_type == CrateType("bin") and common_args.emit == Emit("link"):
        # A rustc-driven binary link is as heavy on the host as a cxx link of
        # the same binary, so it shares the toolchain's link weight.
        weight = compile_ctx.cxx_toolchain_info.linker_info.link_weight
        if not incremental_enabled and link_cxx_binary_locally(ctx):
            prefer_local = True

    if is_clippy:
        category = "clippy"
        identifier = ""
    else:
        category = "rustc"
        if common_args.emit == Emit("miri"):
            category = "miri"
        identifier = _explain(
            crate_type = common_args.crate_type,
            link_strategy = common_args.params.dep_link_strategy,
            emit = common_args.emit,
            infallible_diagnostics = infallible_diagnostics,
            profile_mode = profile_mode,
            advanced_unstable_linking = toolchain_info.advanced_unstable_linking,
        )

    if incremental_enabled:
        if not identifier.endswith("]"):
            identifier += " "
        identifier += "[incr]"

    # None defers the choice to the `buck2.default_allow_cache_upload` config; an
    # explicit False overrides it. Actions without a preference pass None.
    if incremental_enabled:
        # Incremental compilation should not publish any action output to a shared cache:
        # 1. the incremental compilation state is not useful for any other user. unfortunately,
        #    there is no mechanism in buck2 that allows for uploading part of an action output
        #    and not another.
        # 2. even if there were, the rlib is not byte-for-byte reproducible
        #    under `-Cincremental`, even if the source binary is unchanged, because of
        #    https://github.com/rust-lang/rust/pull/139453.
        # For additional context, please see this zulip conversation:
        # https://rust-lang.zulipchat.com/#narrow/channel/131828-t-compiler/topic/Possible.20Reproducibility.20Bug.20in.20Soundness.20Fix.3F/with/616729102.
        action_allow_cache_upload = False
    elif allow_cache_upload:
        action_allow_cache_upload = True
    elif is_clippy:
        # Clippy never uploads.
        action_allow_cache_upload = False
    else:
        # Libraries (check, metadata, rlib) have no preference.
        action_allow_cache_upload = None

    compile_cmd, adapter_options = adapt_rustc_action(
        ctx,
        compile_cmd,
        toolchain_info.rustc_action_adapter,
        "{}-{}".format(prefix, diag),
    )

    ctx.actions.run(
        compile_cmd,
        local_only = local_only,
        prefer_local = prefer_local,
        weight = weight,
        category = category,
        identifier = identifier,
        no_outputs_cleanup = incremental_enabled,
        allow_cache_upload = action_allow_cache_upload,
        error_handler = toolchain_info.rust_error_handler,
        **adapter_options
    )

    return Invoke(
        diag_txt = diag_txt,
        diag_json = diag_json,
        build_status = build_status,
        identifier = identifier,
    )

# Our rustc and rustdoc commands can have arbitrarily large number of `--extern`
# flags, so write to file to avoid hitting the platform's limit on command line
# length. This limit is particularly small on Windows.
def _long_command(ctx: AnalysisContext, exe: RunInfo, args: cmd_args, argfile_name: str, has_content_based_path: bool = False) -> cmd_args:
    return cmd_args(
        exe,
        at_argfile(
            actions = ctx.actions,
            name = argfile_name,
            args = args,
            allow_args = True,
            has_content_based_path = has_content_based_path,
        ),
    )

# An escape per character that cannot sit on one line of the `rustc_action` argfile: the
# backslash first, so that one left-to-right pass decodes, then every separator Python's
# `str.splitlines` recognises. `rustc_action.py` undoes this.
_RUSTC_ACTION_ENV_ESCAPES = [
    ("\\", "\\\\"),
    ("\n", "\\n"),
    ("\r", "\\r"),
    ("\x0b", "\\v"),
    ("\x0c", "\\f"),
    ("\x1c", "\\x1c"),
    ("\x1d", "\\x1d"),
    ("\x1e", "\\x1e"),
    ("\x85", "\\x85"),
    ("\u2028", "\\u2028"),
    ("\u2029", "\\u2029"),
]
_RUSTC_ACTION_ENV_REPLACE_REGEX = [(regex("\\x{%x}" % ord(raw)), escaped) for raw, escaped in _RUSTC_ACTION_ENV_ESCAPES]
_DIRECTORY_ENV = [
    "CARGO_MANIFEST_DIR",
    "OUT_DIR",
]

# Separate env settings into "plain" and "with path". Path env vars are often
# used in Rust `include!()` and similar directives, which always interpret the
# path relative to the source file containing the directive. Since paths in env
# vars are often expanded from macros such as `$(location)`, they will be
# cell-relative which will not work properly. To solve this, we canonicalize
# paths to absolute paths so they'll work in any context. Hence the need to
# distinguish path from non-path. (This will not work if the value contains both
# path and non-path content, but we'll burn that bridge when we get to it.)
def _env_value_needs_escape(value: str) -> bool:
    for raw, _ in _RUSTC_ACTION_ENV_ESCAPES:
        if raw in value:
            return True
    return False

def process_env(
    compile_ctx: CompileContext, env: dict[str, str | ResolvedStringWithMacros | Artifact], escape_for_rustc_action: bool = True
) -> (dict[str, cmd_args | str | ResolvedStringWithMacros], dict[str, cmd_args]):
    # Values with inputs (ie artifact references).
    path_env = {}

    # Plain strings.
    plain_env = {}

    for k, v in env.items():
        if type(v) == "string" and not (escape_for_rustc_action and _env_value_needs_escape(v)):
            plain_env[k] = v
            continue

        arg = cmd_args(v)
        if len(arg.inputs) > 0:
            path_env[k] = arg
        elif escape_for_rustc_action:
            plain_env[k] = cmd_args(v, replace_regex = _RUSTC_ACTION_ENV_REPLACE_REGEX)
        else:
            # No escaping requested and no inputs to track: retain the value
            # itself rather than a single-element `cmd_args` wrapper.
            plain_env[k] = v

    # If CARGO_MANIFEST_DIR is not already expressed in terms of $(location ...)
    # of some target, then interpret it as a relative path inside of the crate's
    # sources.
    #
    # For example in the following case:
    #
    #     http_archive(
    #         name = "foo.crate",
    #         ...
    #     )
    #
    #     rust_library(
    #         name = "foo",
    #         srcs = [":foo.crate"],
    #         crate_root = "foo.crate/src/lib.rs",
    #         env = {
    #             "CARGO_MANIFEST_DIR": "foo.crate",
    #         },
    #     )
    #
    # then the manifest directory refers to the directory which is the parent of
    # `src` inside the archive.
    #
    # By putting the environment variable into path_env, rustc_action.py will
    # take care of turning this into an absolute path before rustc sees it. This
    # matches Cargo which also always provides CARGO_MANIFEST_DIR as an absolute
    # path. A relative path would be problematic because it can't simultaneously
    # support both of the following real-world cases: `include!` which resolves
    # relative paths relative to the file containing the include:
    #
    #     include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/thing.rs"));
    #
    # and proc macros using std::fs to read thing like .pest grammars, which
    # would need paths relative to the directory that rustc got invoked in
    # (which is the repo root in Buck builds).
    for key in _DIRECTORY_ENV:
        value = plain_env.pop(key, None)
        if value != None:
            path_env[key] = cmd_args(
                compile_ctx.symlinked_srcs,
                compile_ctx.path_sep,
                value,
                delimiter = "",
            )

    return (plain_env, path_env)

def _dist_thinlto_enabled(ctx: AnalysisContext, compile_ctx: CompileContext) -> bool:
    if not getattr(ctx.attrs, "enable_distributed_thinlto", False):
        return False
    linker_info = compile_ctx.cxx_toolchain_info.linker_info
    return linker_info.supports_distributed_thinlto and linker_info.lto_mode == LtoMode("thin")

def _base_rustc_env(ctx: AnalysisContext, compile_ctx: CompileContext) -> (dict[str, cmd_args | str], dict[str, cmd_args]):
    cached = compile_ctx.processed_env.get("rustc")
    if cached == None:
        cached = process_env(compile_ctx, compile_ctx.toolchain_info.rustc_env | ctx.attrs.env)
        compile_ctx.processed_env["rustc"] = cached

    # Fresh dicts for the caller to extend with emit-specific values.
    return (dict(cached[0]), dict(cached[1]))

def get_inherited_link_args(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    dep_link_style: LinkStrategy,
    rust_cxx_link_group_info: RustCxxLinkGroupInfo | None,
    transformation_spec_context: TransformationSpecContext | None,
) -> LinkArgs:
    if rust_cxx_link_group_info:
        filtered_links = rust_cxx_link_group_info.filtered_links

        # Unfortunately, link_groups does not use MergedLinkInfo to represent the args
        # for the resolved nodes in the graph.
        # Thus, we have no choice but to traverse all the nodes to dedupe the framework linker args.
        additional_links = apple_get_link_info_by_deduping_link_infos(
            ctx,
            infos = filtered_links,
            framework_linkable = None,
            swiftmodule_linkable = None,
        )
        if additional_links:
            filtered_links.append(additional_links)

        inherited_link_args = LinkArgs(
            infos = filtered_links + [rust_cxx_link_group_info.symbol_files_info],
        )
    else:
        inherited_link_args = apple_build_link_args_with_deduped_flags(
            ctx,
            deps_merged_link_infos = inherited_merged_link_infos(
                ctx,
                compile_ctx.dep_ctx,
            ),
            frameworks_linkable = None,
            link_strategy = dep_link_style,
            swiftmodule_linkable = None,
            prefer_stripped = False,
            transformation_spec_context = transformation_spec_context,
        )

    return inherited_link_args

def rust_link_shared(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    dep_link_style: LinkStrategy,
    static_lib: LinkInfo,
    # Defaults to `shared/<soname>`, carrying the soname as `DT_SONAME`.
    # A Python extension is loaded by file path and has no soname.
    output: str | None = None,
    set_soname: bool = True,
) -> LinkedObject:
    inherited_link_args = get_inherited_link_args(
        ctx,
        compile_ctx,
        dep_link_style,
        rust_cxx_link_group_info = None,
        transformation_spec_context = None,
    )
    link_args = [
        LinkArgs(
            flags = cmd_args(
                compile_ctx.toolchain_info.linker_flags,
                ctx.attrs.linker_flags,
            )
        ),
        # Need link whole because otherwise the linker doesn't include anything at all (normally
        # you'd be passing in `.o`s but that's not really an option)
        #
        # FIXME(JakobDegen): Normally, this is where rustc would include the `symbols.o` and the
        # linker version file/deffile to make sure the right things get linked in and remain
        # visible. However, we don't have those, which might not be great in the case of `symbols.o`
        # and will certainly break things if we try this on Windows.
        #
        # Principally, it would actually be pretty easy to get us access to those. When building the
        # `pic_archive`, we could just pass `--crate-type=dylib` to rustc anyway and use the linker
        # extraction tricks to pull out the files we need (in addition to the object files) to then
        # be consumed here. The only problem is that rustc is 50-100% slower to do dylib builds
        # vs equivalent rlib builds, even without the linking. So until that's fixed we can't really
        # use it.
        LinkArgs(infos = [set_link_info_link_whole(static_lib)]),
        inherited_link_args,
    ]

    return cxx_link_shared_library(
        ctx = ctx,
        output = output or "shared/" + compile_ctx.soname,
        opts = link_options(
            links = link_args,
            link_execution_preference = LinkExecutionPreference("any"),
            category_suffix = "rust_shared",
            # `rust_library` has no `allow_cache_upload` attribute
            allow_cache_upload = None,
        ),
        name = compile_ctx.soname if set_soname else None,
    ).linked_object

def rust_link_binary(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    extraction: LinkExtraction,
    dep_link_strategy: LinkStrategy,
    reloc_model: RelocModel,
    extra_link_args: list[typing.Any],
    binary_link_args: list[typing.Any],
    inherited_link_args: LinkArgs,
    dwo_output_directory: Artifact | None,
    output: Artifact,
    output_has_content_based_path: bool,
    identifier: str | None,
    allow_cache_upload: bool,
) -> CxxLinkResult:
    """Link an executable from the objects that a bin-crate `Emit("rlib")`
    `rust_compile` extracted, plus the link args of the dependency graph."""
    linker_flags = ctx.attrs.linker_flags
    if getattr(ctx.attrs, "_generated_build_info_enabled", False):
        linker_flags = strip_build_info_linker_flags(linker_flags)
    dist_thinlto = extraction.out_archive != None

    retained_flags = cmd_args(extraction.out_argsfile, format = "@{}")
    if not dist_thinlto:
        # Which objects rustc produced is only known once it has run, so the
        # argsfile listing them is written by a dynamic action after the
        # manifest is available. Resolving the manifest's names against the
        # artifacts directory here — rather than having the extraction write
        # paths into a file itself — keeps the paths buck-rendered, which is
        # what makes them correct under content-based paths.
        objects_argsfile = ctx.actions.declare_output(output.short_path + ".rust_objects.args", has_content_based_path = False)

        def write_objects_argsfile(ctx: AnalysisContext, artifacts, outputs):
            names = artifacts[extraction.out_manifest].read_string().splitlines()
            ctx.actions.write(
                outputs[objects_argsfile],
                cmd_args([extraction.out_artifacts_dir.project(name) for name in names]),
            )

        ctx.actions.dynamic_output(
            dynamic = [extraction.out_manifest],
            inputs = [],
            outputs = [objects_argsfile.as_output()],
            f = write_objects_argsfile,
        )

        rust_objects = LinkArgs(
            flags = [
                retained_flags,
                cmd_args(objects_argsfile, format = "@{}", hidden = extraction.out_artifacts_dir),
            ]
        )
    else:
        rust_objects = LinkArgs(
            infos = [
                LinkInfo(
                    name = "rust_objects",
                    # The opt actions re-run codegen and take their relocation
                    # model from their own command line, so they must match the
                    # relocation model rustc's own codegen used: PIC for every
                    # strategy but `static` (see `_get_reloc_model`).
                    dist_thin_lto_codegen_flags = (compile_ctx.toolchain_info.dist_thin_lto_codegen_flags if dep_link_strategy != LinkStrategy("static") else []),
                    pre_flags = [retained_flags],
                    linkables = [
                        ArchiveLinkable(
                            archive = Archive(artifact = extraction.out_archive),
                            linker_type = compile_ctx.cxx_toolchain_info.linker_info.type,
                            # link_whole because everything rustc emits has to end up in the input.
                            link_whole = True,
                        )
                    ],
                )
            ]
        )

    split_debug_mode = compile_ctx.cxx_toolchain_info.split_debug_mode or SplitDebugMode("none")
    if split_debug_mode != SplitDebugMode("none"):
        inherited_debug_info = inherited_external_debug_info(
            ctx = ctx,
            dep_ctx = compile_ctx.dep_ctx,
            dep_link_strategy = dep_link_strategy,
        )
    else:
        inherited_debug_info = ArtifactTSet()
    external_debug_info = make_artifact_tset(
        actions = ctx.actions,
        label = ctx.label,
        artifacts = [dwo_output_directory] if dwo_output_directory else [],
        children = [inherited_debug_info],
    )

    links = [
        # The flags this link needs beyond what `cxx_link` applies on its own;
        # `binary_linker_flags` mirrors `cxx_executable`'s use of them.
        LinkArgs(
            flags = cmd_args(
                compile_ctx.cxx_toolchain_info.linker_info.binary_linker_flags,
                compile_ctx.toolchain_info.linker_flags,
                linker_flags,
            )
        ),
        LinkArgs(flags = extra_link_args),
    ]

    if compile_ctx.cxx_toolchain_info.linker_info.type == LinkerType("gnu"):
        flags = [
            # FIXME(JakobDegen): This is here becuase rustc used to pass it, but it's not something
            # we should be passing blindly on the user's behalf. Unfortunately some builds break if
            # you take it out.
            "-Wl,--as-needed",
            # lld deduplicates `.debug_str` only at `-O1`, and rustc always
            # passes this for links it drives, overriding toolchains that link
            # at `-O0` for speed (fbcode's opt-lg). Without the deduplication,
            # very large binaries overflow the 32-bit `.debug_str` offsets in
            # `.debug_names`.
            "-Wl,-O1",
            # The objects were codegened at the relocation model chosen in
            # `_get_reloc_model`, which the toolchain's link flags cannot know
            # about. Pin the PIE-ness of the executable to match, as rustc does
            # for links it drives.
            "-no-pie" if reloc_model == RelocModel("static") else "-pie",
        ]

        # On ELF, rustc-instrumented objects carry no reference to the
        # profile runtime's registration object (clang-instrumented ones
        # do), so nothing would pull it out of the `profiler_builtins`
        # archive and the binary would silently write no profile. rustc
        # compensates with this flag on links it drives; do the same,
        # gated exactly like the `-Cinstrument-coverage` the compile gets.
        if getattr(ctx.attrs, "coverage", False) and compile_ctx.toolchain_info.rustc_coverage_flags:
            flags.append("-Wl,-u,__llvm_profile_runtime")

        links.append(LinkArgs(flags = flags))

    links += [
        rust_objects,
        inherited_link_args,
        LinkArgs(infos = [LinkInfo(external_debug_info = external_debug_info)]),
    ]

    link_execution_preference = get_link_execution_preference(ctx, [])
    if link_execution_preference == LinkExecutionPreference("any") and link_cxx_binary_locally(ctx, compile_ctx.cxx_toolchain_info):
        link_execution_preference = LinkExecutionPreference("local")

    return cxx_link_into(
        ctx = ctx,
        output = output,
        result_type = CxxLinkResultType("executable"),
        output_has_content_based_path = output_has_content_based_path,
        opts = link_options(
            binary_links = [LinkArgs(flags = binary_link_args)] if binary_link_args else [],
            links = links,
            link_execution_preference = link_execution_preference,
            # The link is local only so the fbcode linker wrapper can stamp
            # build info from the repo (D115067153); dwp needs no repo and is
            # memory-hungry enough that pinning it to the link's host thrashes
            # small workers. Let it schedule anywhere, matching the
            # rustc-linked path, which runs dwp with no preference.
            dwp_execution_preference = LinkExecutionPreference("any"),
            category_suffix = "rust_binary",
            identifier = identifier,
            enable_distributed_thinlto = dist_thinlto,
            allow_cache_upload = allow_cache_upload,
        ),
    )
