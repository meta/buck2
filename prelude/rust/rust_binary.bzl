# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "project_artifacts",
)
load("@prelude//:command_alias.bzl", "command_alias")
load("@prelude//:content_based_dist.bzl", "make_content_based_dist")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load(
    "@prelude//:resources.bzl",
    "create_relocatable_resources_info",
    "create_resource_db",
    "gather_resources",
)
load("@prelude//cxx:attrs.bzl", "BUILD_INFO_ATTR")
load("@prelude//cxx:cxx_bolt.bzl", "cxx_use_bolt")
load("@prelude//cxx:cxx_executable.bzl", "get_cxx_post_link_suffix")
load(
    "@prelude//cxx:cxx_library_utility.bzl",
    "cxx_attr_deps",
)
load("@prelude//cxx:cxx_link_utility.bzl", "executable_shared_lib_arguments")
load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "PicBehavior",
    "RuntimeDependencyHandling",
)
load("@prelude//cxx:cxx_utility.bzl", "cxx_attrs_get_allow_cache_upload")
load(
    "@prelude//cxx:link_groups.bzl",
    "LINK_GROUP_MAPPINGS_FILENAME_SUFFIX",
    "LINK_GROUP_MAPPINGS_SUB_TARGET",
    "LINK_GROUP_MAP_DATABASE_SUB_TARGET",
    "LinkGroupContext",
    "build_shared_libs_for_symlink_tree",
    "get_link_group_map_json",
)
load("@prelude//cxx:linker.bzl", "DUMPBIN_SUB_TARGET", "PDB_SUB_TARGET", "get_dumpbin_providers", "get_pdb_providers")
load("@prelude//cxx:transformation_spec.bzl", "build_transformation_spec_context")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:native_common.bzl", "native_common")
load("@prelude//decls:re_test_common.bzl", "re_test_common")
load("@prelude//decls:test_common.bzl", "test_common")
load(
    "@prelude//dist:dist_info.bzl",
    "DistInfo",
)
load("@prelude//linking:add_elf_sections.bzl", "get_elf_sections")
load("@prelude//linking:generated_build_info.bzl", "compile_generated_build_info", "generate_build_info")
load(
    "@prelude//linking:link_info.bzl",
    "LibOutputStyle",
    "LinkStrategy",
    "get_lib_output_style",
    "process_link_strategy_for_pic_behavior",
    "unpack_link_args",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "create_linkable_graph",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "merge_shared_libraries",
    "traverse_shared_library_info",
)
load("@prelude//linking:stamp_build_info.bzl", "cxx_stamp_build_info", "stamp_build_info")
load("@prelude//os_lookup:defs.bzl", "OsLookup")
load(
    "@prelude//rust:attrs.bzl",
    "RUST_EXECUTABLE_ATTRIBUTES",
    "miri_interpreted_attr",
    "rust_analyzer_target_kind",
    "rust_binary_attrs_group",
    "rust_common",
    "rust_common_attributes",
)
load("@prelude//rust:link_info.bzl", "RustProcMacroPlugin")
load("@prelude//rust/rust-analyzer:provider.bzl", "rust_analyzer_provider")
load("@prelude//test:inject_test_run_info.bzl", "inject_test_run_info")
load(
    "@prelude//tests:re_utils.bzl",
    "get_re_executors_from_props",
)
load(
    "@prelude//utils:build_graph_pattern.bzl",
    "new_build_graph_info",
)
load("@prelude//utils:utils.bzl", "flatten_dict")
load(
    ":build.bzl",
    "generate_rustdoc",
    "get_inherited_link_args",
    "miri_command",
    "rust_compile",
    "rust_compile_invalidation_inputs",
    "rust_link_binary",
)
load(
    ":build_params.bzl",
    "BuildParams",  # @unused Used as a type
    "Emit",
    "ProfileMode",  # @unused Used as a type
    "binary_build_params",
)
load(
    ":context.bzl",
    "CompileContext",
    "compile_context",
    "output_filename",
)
load(
    ":link_info.bzl",
    "DEFAULT_STATIC_LINK_STRATEGY",
    "attr_simple_crate_for_filenames",
    "inherited_external_debug_info",
    "inherited_linkable_graphs",
    "inherited_rust_cxx_link_group_info",
    "inherited_shared_libs",
)
load(":named_deps.bzl", "write_named_deps_names")
load(":outputs.bzl", "RustcExtraOutputsInfo", "output_as_diag_subtargets")
load(":profile.bzl", "make_profile_providers")
load(":resources.bzl", "rust_attr_resources")
load(":rust_toolchain.bzl", "PanicRuntime", "RustToolchainInfo", "toolchain_supports_miri")

def _get_runtime_dependency_handling(ctx: AnalysisContext, compile_ctx: CompileContext) -> RuntimeDependencyHandling:
    """Read runtime_dependency_handling from attribute, falling back to CXX toolchain default."""
    if ctx.attrs.runtime_dependency_handling:
        return RuntimeDependencyHandling(ctx.attrs.runtime_dependency_handling)

    # Toolchains that don't support shared libraries (e.g. WASM, bare-metal)
    # cannot create symlink trees, so skip runtime dependency handling.
    if not compile_ctx.cxx_toolchain_info.linker_info.supports_shared_libraries:
        return RuntimeDependencyHandling("no_symlink")
    return compile_ctx.cxx_toolchain_info.runtime_dependency_handling

def _strategy_params(ctx: AnalysisContext, compile_ctx: CompileContext) -> dict[LinkStrategy, BuildParams]:
    target_os_type = ctx.attrs._target_os_type[OsLookup]
    linker_type = compile_ctx.cxx_toolchain_info.linker_info.type

    params = {}
    for link_strategy in LinkStrategy:
        params[link_strategy] = binary_build_params(
            proc_macro = False,
            link_strategy = link_strategy,
            linker_type = linker_type,
            target_os_type = target_os_type,
        )
    return params

def _create_content_based_dist(
    ctx: AnalysisContext,
    name: str,
    exe: Artifact,
    relocatable_resources_json: Artifact | None,
    relocatable_resources_contents: Artifact | None,
    shlib_tree: Artifact | list | None,
) -> (Artifact, Artifact):
    """
    Build the content-based `assembled_dir` bundle for a `rust_binary` that opts
    into `has_content_based_path` and has adjacent files: the relocatable resource
    DB (copied) + the content-based `__resources` dir (symlinked), and the shlib
    symlink-tree (symlinked). The exe and the copy/symlink contract are handled by
    `make_content_based_dist`. Returns `(bundle_dir, projected exe)`.
    """
    exe_rel = exe.short_path
    copies = {}
    symlinks = {}

    # Resources: reuse the relocatable DB + `__<name>__resources` symlink dir
    # from `create_relocatable_resources_info` -- which is CONTENT-BASED for this
    # binary (rust_binary threads `has_content_based_path` through). Copy the DB
    # (its bytes are captured in the bundle); symlink the content-based resource
    # dir. The dir being content-based is load-bearing: its path is immutable and
    # re-hashes when a content-based resource changes, so the bundle's symlink to
    # it is trample-safe -- unlike the config-based default, which would be
    # rewritten in place under a running bundle.
    if relocatable_resources_json != None:
        copies[exe_rel + ".resources.json"] = relocatable_resources_json
        symlinks[relocatable_resources_contents.short_path] = relocatable_resources_contents

    # Shared libraries: symlink the whole prebuilt shlib symlink-tree into the
    # bundle at the `$ORIGIN/<tree>`-relative offset the exe's RPATH already
    # encodes (`get_rpath_origin` => `$ORIGIN` for the gnu linker). Symlinking
    # the tree the linker already targets -- rather than reconstructing it
    # per-soname -- reuses the tree tool's exact layout, so binaries with
    # computed (non-string) sonames are handled too, and the loader follows the
    # symlinked dir like any RPATH entry.
    if isinstance(shlib_tree, list) and shlib_tree:
        # Windows produces a list of per-DLL symlink artifacts (no `$ORIGIN`
        # RPATH); co-locating them here is unimplemented. Fail loudly rather
        # than ship a bundle that silently can't find its DLLs at runtime.
        fail(
            "content-based `assembled_dir` bundle does not support a Windows shared-library tree; a content-based rust_binary with shared libraries is not supported on Windows yet"
        )
    if isinstance(shlib_tree, Artifact):
        symlinks[shlib_tree.short_path] = shlib_tree

    return make_content_based_dist(ctx, name, exe, copies = copies, symlinks = symlinks)

def _native_test_info(
    ctx: AnalysisContext,
    args: cmd_args,
    re_executors,
) -> list[Provider]:
    """Default (native) test execution for a test rule."""
    return inject_test_run_info(
        ctx,
        ExternalRunnerTestInfo(
            type = "rust",
            command = [args],
            env = ctx.attrs.env | ctx.attrs.run_env,
            labels = ctx.attrs.labels,
            contacts = ctx.attrs.contacts,
            default_executor = re_executors.default_executor,
            executor_overrides = re_executors.executor_overrides,
            run_from_project_root = True,
            use_project_relative_paths = True,
            supports_test_execution_caching = ctx.attrs.supports_test_execution_caching,
        ),
    )

def _miri_test_info(
    ctx: AnalysisContext,
    miri_direct_cmd: cmd_args,
    re_executors,
) -> ExternalRunnerTestInfo:
    """Miri test execution for the `[miri]` sub-target, or the target itself when `miri_interpreted`."""
    # Compile-time env rides inside the wrapped command (`--env`/`--path-env`),
    # so no test-info env is needed.
    labels = list(ctx.attrs.labels or [])
    if "miri" not in labels:
        labels.append("miri")
    return ExternalRunnerTestInfo(
        type = "rust",
        command = [miri_direct_cmd],
        env = {},
        labels = labels,
        contacts = ctx.attrs.contacts,
        default_executor = re_executors.default_executor,
        executor_overrides = re_executors.executor_overrides,
        run_from_project_root = True,
        use_project_relative_paths = True,
    )

def _miri_direct_command(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    toolchain_info: RustToolchainInfo,
    default_roots: list[str],
    extra_flags: list[str | ResolvedStringWithMacros | Artifact],
    runtime_env: dict[str, str | ResolvedStringWithMacros | Artifact],
    is_test: bool,
    strategy_params: dict[LinkStrategy, BuildParams],
) -> cmd_args | None:
    """Wrapped Miri driver command, or `None` without a Miri toolchain."""
    if not toolchain_supports_miri(toolchain_info):
        return None
    return miri_command(
        ctx = ctx,
        compile_ctx = compile_ctx,
        params = strategy_params[DEFAULT_STATIC_LINK_STRATEGY],
        default_roots = default_roots,
        extra_flags = extra_flags,
        runtime_env = runtime_env,
        is_test = is_test,
    )

def _miri_launcher(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    miri_direct_cmd: cmd_args,
) -> list[Provider]:
    """Executable launcher for a Miri run: the `[miri]` sub-target, or the target itself when `miri_interpreted`."""
    # Env rides inside the wrapped command, so the launcher is just the command.
    base = RunInfo(args = miri_direct_cmd)
    args = cmd_args()
    launcher = command_alias(
        actions = ctx.actions,
        path = "miri_launcher",
        target_os = compile_ctx.exec_os_type,
        base = base,
        args = args,
        env = {},
        labels = [],
        has_content_based_path = False,
    )
    return [
        launcher.output,
        RunInfo(args = launcher.cmd),
    ]

def _miri_test_sub_targets(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    toolchain_info: RustToolchainInfo,
    miri_direct_cmd: cmd_args,
    re_executors,
) -> list[Provider]:
    """Launcher plus test info for an interpreted test run: the `[miri]` sub-target, or the target itself when `miri_interpreted`."""
    # The launcher serves `buck2 run`, where no runner adds
    # `-Zunstable-options` to unlock `--force-run-in-process`; the test
    # info omits it because TPX supplies it.
    launcher_cmd = miri_direct_cmd
    if toolchain_info.panic_runtime == PanicRuntime("abort"):
        launcher_cmd = cmd_args(miri_direct_cmd, "-Zunstable-options")
    return _miri_launcher(ctx, compile_ctx, launcher_cmd) + [
        _miri_test_info(ctx, miri_direct_cmd, re_executors),
    ]

# Native link outputs shared by the binary and test impls.
NativeBinary = record(
    providers = field(list[Provider]),
    default_info = field(DefaultInfo),
    args = field(cmd_args),
    sub_targets = field(dict[str, list[Provider]]),
)

def _rust_binary_common(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    default_roots: list[str],
    extra_flags: list[str | ResolvedStringWithMacros | Artifact],
    allow_cache_upload: bool,
    strategy_params: dict[LinkStrategy, BuildParams],
    miri_sub_targets = None,
) -> NativeBinary:
    toolchain_info = compile_ctx.toolchain_info

    simple_crate = attr_simple_crate_for_filenames(ctx)

    link_strategy = LinkStrategy(ctx.attrs.link_style) if ctx.attrs.link_style else DEFAULT_STATIC_LINK_STRATEGY
    link_strategy = process_link_strategy_for_pic_behavior(link_strategy, compile_ctx.cxx_toolchain_info.pic_behavior)

    cxx_deps = cxx_attr_deps(ctx)
    generated_build_info_enabled = getattr(ctx.attrs, "_generated_build_info_enabled", False)
    generated_build_info_args = ctx.attrs._generated_build_info_args if generated_build_info_enabled else []

    generated_build_info_link_args = []
    generated_build_info_link_objects = []
    generated_build_info_shared_library_link_args = []

    resources = flatten_dict(
        gather_resources(
            label = ctx.label,
            resources = rust_attr_resources(ctx),
            deps = cxx_deps,
        ).values()
    )

    params = strategy_params[link_strategy]
    name = output_filename(compile_ctx, simple_crate, Emit("link"), params)

    # Under advanced_unstable_linking, the binary is compiled via
    # `Emit("rlib")` and linked through cxx below (`rust_link_binary`),
    # mirroring how rust_library produces dylibs via `rust_link_shared`.
    links_via_cxx = toolchain_info.advanced_unstable_linking
    bin_emit = Emit("rlib") if links_via_cxx else Emit("link")

    use_bolt = cxx_use_bolt(ctx)
    if use_bolt and not links_via_cxx:
        fail(
            "`bolt_profile` is set, but BOLT runs as part of the cxx link. "
            + "This target links via rustc; BOLT requires `advanced_unstable_linking` on the Rust toolchain."
        )
    elf_sections = get_elf_sections(ctx)
    if elf_sections and not links_via_cxx:
        fail(
            "`elf_sections` is set, but the sections are added as part of the cxx link. "
            + "This target links via rustc; `elf_sections` requires `advanced_unstable_linking` on the Rust toolchain."
        )
    enable_late_build_info_stamping = cxx_stamp_build_info(ctx)
    content_based_output = getattr(ctx.attrs, "has_content_based_path", False)
    unstamped_name = None
    if enable_late_build_info_stamping:
        allow_cache_upload = True
        unstamped_name = output_filename(compile_ctx, simple_crate, Emit("link"), params, "-unstamped")

    # The exe output is declared below, once we know whether it ships in a
    # content-based `copied_dir` bundle (see the `will_bundle` decision after
    # `shared_libs` is computed).

    build_graph_info = new_build_graph_info(ctx, cxx_deps)
    transformation_spec_context = build_transformation_spec_context(ctx, build_graph_info)

    rust_cxx_link_group_info = inherited_rust_cxx_link_group_info(
        ctx,
        compile_ctx.dep_ctx,
        link_strategy = link_strategy,
        transformation_spec_context = transformation_spec_context,
    )

    # Link groups repartition the dependency graph's code — including the
    # Rust code — into group libraries, filtering the main link accordingly.
    # Without advanced_unstable_linking, rustc links the full dep rlib set no
    # matter what, so the grouped code would be linked twice; this cannot be
    # made correct.
    if rust_cxx_link_group_info != None and not toolchain_info.advanced_unstable_linking:
        fail("link groups on Rust binaries require `advanced_unstable_linking`")
    if rust_cxx_link_group_info != None:
        link_group_mappings = rust_cxx_link_group_info.link_group_info.mappings
        link_group_libs = rust_cxx_link_group_info.link_group_libs
        link_group_preferred_linkage = rust_cxx_link_group_info.link_group_preferred_linkage
        labels_to_links_map = rust_cxx_link_group_info.labels_to_links_map
        targets_consumed_by_link_groups = rust_cxx_link_group_info.targets_consumed_by_link_groups
        filtered_targets = rust_cxx_link_group_info.filtered_targets
    else:
        link_group_mappings = {}
        link_group_libs = {}
        link_group_preferred_linkage = {}
        labels_to_links_map = {}
        targets_consumed_by_link_groups = {}
        filtered_targets = set()

    native_link_args = get_inherited_link_args(
        ctx,
        compile_ctx,
        params.dep_link_strategy,
        rust_cxx_link_group_info,
        transformation_spec_context,
    )

    runtime_dep_handling = _get_runtime_dependency_handling(ctx, compile_ctx)

    shlib_deps = []
    if link_strategy == LinkStrategy("shared") or rust_cxx_link_group_info != None:
        shlib_deps = inherited_shared_libs(ctx, compile_ctx.dep_ctx)
    elif runtime_dep_handling == RuntimeDependencyHandling("symlink"):
        # Include all transitive runtime shared library deps in a symlink tree.
        # Only collect libs that would actually be built as shared libraries
        # given the current link strategy, matching cxx_binary behavior.
        linkable_graphs_tset = inherited_linkable_graphs(ctx, compile_ctx.dep_ctx)
        all_graphs = []
        for graphs in linkable_graphs_tset.traverse():
            all_graphs.extend(graphs)
        linkable_graph = create_linkable_graph(ctx, deps = all_graphs)
        for linkable_node in linkable_graph.nodes.traverse():
            if linkable_node.linkable == None:
                continue
            preferred_linkage = linkable_node.linkable.preferred_linkage
            output_style = get_lib_output_style(link_strategy, preferred_linkage, PicBehavior("supported"))
            if output_style == LibOutputStyle("shared_lib") and not linkable_node.linkable.stub:
                shlib_deps.append(merge_shared_libraries(ctx.actions, node = linkable_node.linkable.shared_libs))

    shlib_info = merge_shared_libraries(ctx.actions, deps = shlib_deps)

    link_group_ctx = LinkGroupContext(
        link_group_mappings = link_group_mappings,
        link_group_libs = link_group_libs,
        link_group_preferred_linkage = link_group_preferred_linkage,
        labels_to_links_map = labels_to_links_map,
        targets_consumed_by_link_groups = targets_consumed_by_link_groups,
    )

    # Gather and setup symlink tree of transitive shared library deps.
    shared_libs = build_shared_libs_for_symlink_tree(
        use_link_groups = rust_cxx_link_group_info != None,
        link_group_ctx = link_group_ctx,
        link_strategy = link_strategy,
        shared_libraries = traverse_shared_library_info(shlib_info, transformation_provider = transformation_spec_context),
        extra_shared_libraries = [],
    )

    # Decide whether the content-based output ships as a bare exe or a
    # `assembled_dir` bundle (see `_create_content_based_dist`). A binary with
    # adjacent files -- resources and/or a shared-lib tree -- goes in a bundle,
    # and its exe must stay config-based: a content-based exe sits at a deep
    # content-hash path, so the linker bakes a long buck-out-relative `$ORIGIN`
    # RPATH that no longer points at the shlib tree once the exe is copied into
    # the bundle. A config-based exe is a direct sibling of the tree, yielding a
    # clean `$ORIGIN/<tree>` RPATH that survives co-location; the bundle dir
    # provides the content-addressing instead.
    # `needs_shlib_tree` = a shlib symlink-tree (and a baked `$ORIGIN/<tree>`
    # RPATH) was produced for this exe. That is exactly when `shared_libs` is
    # non-empty: `link_strategy == shared`, a link-group binary (auto link groups
    # are on by DEFAULT, so `link_strategy == shared` alone is NOT sufficient --
    # a static_pic link-group binary still gets a tree + RPATH), or
    # `runtime_dependency_handling = "symlink"` dlopen deps. In every such case
    # the exe carries an `$ORIGIN/<tree>` RPATH, so a content-based (deep-hash)
    # exe would bake a dangling RPATH; bundle, and keep the exe config-based, so
    # the RPATH resolves against the co-located tree. A binary with NO shared
    # libs (e.g. the fully-static node) has no tree/RPATH and stays a bare
    # content-based exe.
    needs_shlib_tree = bool(shared_libs)
    will_bundle = content_based_output and (bool(resources) or needs_shlib_tree)

    # The bare exe stays content-based -- so it is trample-safe when consumed by
    # path or as another target's resource (e.g. clio's `:web-server` embeds the
    # node) -- EXCEPT when it needs a shlib tree at startup. A content-based exe
    # sits at a deep content-hash path, so the linker bakes a long
    # buck-out-relative `$ORIGIN` RPATH that no longer points at the tree once
    # the exe is copied into the bundle. Dynamic binaries therefore use a
    # config-based exe (a direct sibling of the tree => clean `$ORIGIN/<tree>`
    # RPATH that survives co-location) and get content-addressing from the
    # bundle dir instead. Static binaries reuse their content-based exe in the
    # bundle directly (no RPATH to break).
    exe_content_based = content_based_output and not needs_shlib_tree and not use_bolt
    if links_via_cxx:
        # cxx performs the terminal link (see `rust_link_binary` below) and its
        # own post-link processing, so the output has to be named the way those
        # stages expect; we must not stamp again here.
        predeclared_output = ctx.actions.declare_output(
            output_filename(compile_ctx, simple_crate, Emit("link"), params, get_cxx_post_link_suffix(ctx)),
            has_content_based_path = exe_content_based,
        )

        # final_output is whatever cxx returns, set after the link below.
        final_output = None
        # rpath and symlink-tree are computed against the pre-stamped output
        shlib_args_output = predeclared_output
    elif enable_late_build_info_stamping:
        predeclared_output = ctx.actions.declare_output(unstamped_name, has_content_based_path = exe_content_based)
        final_output = ctx.actions.declare_output(name, has_content_based_path = exe_content_based)
        shlib_args_output = final_output
    else:
        # If not using late build info stamping, then the output will be stamped eagerly in rust_compile
        predeclared_output = ctx.actions.declare_output(name, has_content_based_path = exe_content_based)
        final_output = predeclared_output
        shlib_args_output = final_output

    # link groups shared libraries link args are directly added to the link command,
    # we don't have to add them here
    executable_shlib_args = executable_shared_lib_arguments(
        ctx,
        compile_ctx.cxx_toolchain_info,
        shlib_args_output,
        shared_libs,
    )
    if generated_build_info_enabled and not links_via_cxx:
        generated_build_info = generate_build_info(
            ctx,
            generator_args = generated_build_info_args,
            invalidation_inputs = [
                rust_compile_invalidation_inputs(
                    ctx = ctx,
                    compile_ctx = compile_ctx,
                    emit = bin_emit,
                    params = params,
                    default_roots = default_roots,
                    incremental_enabled = ctx.attrs.incremental_enabled,
                ),
                unpack_link_args(native_link_args),
                executable_shlib_args.extra_link_args,
            ],
        )
        if generated_build_info:
            generated_build_info_link_args.extend(generated_build_info.linker_flags)
            generated_build_info_link_args.extend(compile_generated_build_info(ctx, generated_build_info).objects)
    extra_link_args = executable_shlib_args.extra_link_args + generated_build_info_shared_library_link_args + generated_build_info_link_args

    # Compile rust binary. Under `Emit("rlib")`, this only compiles: rustc's
    # synthesized objects are extracted and linked below.
    link = rust_compile(
        ctx = ctx,
        compile_ctx = compile_ctx,
        emit = bin_emit,
        params = params,
        default_roots = default_roots,
        extra_link_args = extra_link_args,
        predeclared_output = None if links_via_cxx else predeclared_output,
        predeclared_output_has_content_based_path = exe_content_based,
        extra_flags = extra_flags,
        allow_cache_upload = allow_cache_upload,
        transformation_spec_context = transformation_spec_context,
        incremental_enabled = ctx.attrs.incremental_enabled,
        precomputed_inherited_link_args = native_link_args,
    )

    prebolt_output = None
    if links_via_cxx:
        extraction = link.product
        rust_link_inputs = [
            extraction.out_argsfile,
            extraction.out_artifacts_dir,
            extraction.out_manifest,
        ]
        if extraction.out_archive != None:
            rust_link_inputs.append(extraction.out_archive)
        if generated_build_info_enabled:
            generated_build_info = generate_build_info(
                ctx,
                generator_args = generated_build_info_args,
                invalidation_inputs = rust_link_inputs
                + [
                    unpack_link_args(native_link_args),
                    executable_shlib_args.extra_link_args,
                ],
            )
            if generated_build_info:
                generated_build_info_link_args.extend(generated_build_info.linker_flags)
                generated_build_info_link_objects.extend(compile_generated_build_info(ctx, generated_build_info).objects)
        link_result = rust_link_binary(
            ctx = ctx,
            compile_ctx = compile_ctx,
            extraction = extraction,
            dep_link_strategy = params.dep_link_strategy,
            reloc_model = params.reloc_model,
            extra_link_args = executable_shlib_args.extra_link_args + generated_build_info_shared_library_link_args,
            binary_link_args = generated_build_info_link_objects + generated_build_info_link_args,
            inherited_link_args = native_link_args,
            dwo_output_directory = link.compile_output.dwo_output_directory,
            output = predeclared_output,
            output_has_content_based_path = exe_content_based,
            identifier = name,
            allow_cache_upload = allow_cache_upload,
        )
        final_output = link_result.linked_object.output
        dwp_output = link_result.linked_object.dwp
        pdb_output = link_result.linked_object.pdb
        prebolt_output = link_result.linked_object.prebolt_output
    else:
        dwp_output = link.product.dwp_output
        pdb_output = link.product.pdb
        if enable_late_build_info_stamping:
            stamp_build_info(ctx, link.product.output, final_output)

    args = cmd_args(final_output, hidden = executable_shlib_args.runtime_files)
    external_debug_info = project_artifacts(
        actions = ctx.actions,
        tsets = inherited_external_debug_info(
            ctx,
            compile_ctx.dep_ctx,
            link_strategy,
        ),
    )
    if link.compile_output.dwo_output_directory:
        external_debug_info.append(link.compile_output.dwo_output_directory)

    # If we have some resources, write it to the resources JSON file and add
    # it and all resources to "runtime_files" so that we make to materialize
    # them with the final binary.
    runtime_files = list(executable_shlib_args.runtime_files)
    relocatable_resources_json = None
    relocatable_resources_contents = None
    if resources:
        resources_hidden = [
            create_resource_db(
                ctx = ctx,
                name = name + ".resources.json",
                binary = final_output,
                resources = resources,
            )
        ]
        for resource in resources.values():
            resources_hidden.append(resource.default_output)
            resources_hidden.extend(resource.other_outputs)
        args.add(cmd_args(hidden = resources_hidden))
        runtime_files.extend(resources_hidden)
        relocatable_resources_json, relocatable_resources_contents = create_relocatable_resources_info(
            ctx = ctx,
            name = name,
            resources = resources,
            has_content_based_path = content_based_output,
        )

    # If opted into content-based output and the binary has adjacent files
    # (resources and/or a shared-lib tree), package everything into a single
    # content-based `assembled_dir` and point both the run command and
    # `DefaultInfo.default_output` at the exe projected out of the bundle, so
    # by-path and resource consumers get the self-contained, co-located form.
    dist_bundle = None
    dist_exe = None
    if will_bundle:
        dist_bundle, dist_exe = _create_content_based_dist(
            ctx = ctx,
            name = name,
            exe = final_output,
            relocatable_resources_json = relocatable_resources_json,
            relocatable_resources_contents = relocatable_resources_contents,
            shlib_tree = executable_shlib_args.shared_libs_symlink_tree,
        )
        # Run the bundled exe (projected out of the content-based bundle). The
        # bundle carries the co-located resources / shlib tree; `runtime_files`
        # is still added so a resource's own `other_outputs` (e.g. a resource
        # that is itself a dynamic exe) materialize too, mirroring the
        # non-bundle contract.
        args = cmd_args(dist_exe, hidden = [dist_bundle] + runtime_files)

    # A simple dict of sub-target key to artifact, which we'll convert to
    # DefaultInfo providers at the end
    extra_compiled_targets = {
        "sources": compile_ctx.symlinked_srcs,
    }
    sub_targets = {}

    # TODO(agallagher) There appears to be pre-existing soname conflicts
    # when building this (when using link groups), which prevents using
    # `with_unique_str_sonames`.
    str_soname_shlibs = {shlib.soname.ensure_str(): shlib for shlib in shared_libs if shlib.soname.is_str}
    sub_targets["shared-libraries"] = [
        DefaultInfo(
            default_output = ctx.actions.write_json(
                name + ".shared-libraries.json",
                {
                    "libraries": ["{}:{}[shared-libraries][{}]".format(ctx.label.path, ctx.label.name, soname) for soname in str_soname_shlibs],
                    "librariesdwp": [
                        "{}:{}[shared-libraries][{}][dwp]".format(ctx.label.path, ctx.label.name, soname)
                        for soname, shlib in str_soname_shlibs.items()
                        if shlib.lib.dwp
                    ],
                    "rpathtree": ["{}:{}[rpath-tree]".format(ctx.label.path, ctx.label.name)] if executable_shlib_args.shared_libs_symlink_tree else [],
                },
                has_content_based_path = False,
            ),
            sub_targets = {
                soname: [
                    DefaultInfo(
                        default_output = shlib.lib.output,
                        sub_targets = {"dwp": [DefaultInfo(default_output = shlib.lib.dwp)]} if shlib.lib.dwp else {},
                    )
                ]
                for soname, shlib in str_soname_shlibs.items()
            },
        )
    ]

    if isinstance(executable_shlib_args.shared_libs_symlink_tree, Artifact):
        sub_targets["rpath-tree"] = [
            DefaultInfo(
                default_output = executable_shlib_args.shared_libs_symlink_tree,
                other_outputs = [shlib.lib.output for shlib in shared_libs] + [shlib.lib.dwp for shlib in shared_libs if shlib.lib.dwp],
            )
        ]

    if rust_cxx_link_group_info:
        sub_targets[LINK_GROUP_MAP_DATABASE_SUB_TARGET] = [get_link_group_map_json(ctx, filtered_targets)]
        readable_mappings = {}
        for node, group in link_group_mappings.items():
            readable_mappings[group] = readable_mappings.get(group, []) + ["{}//{}:{}".format(node.cell, node.package, node.name)]
        sub_targets[LINK_GROUP_MAPPINGS_SUB_TARGET] = [
            DefaultInfo(
                default_output = ctx.actions.write_json(
                    name + LINK_GROUP_MAPPINGS_FILENAME_SUFFIX,
                    readable_mappings,
                    has_content_based_path = False,
                ),
            )
        ]

    # `infallible_diagnostics` allows us to circumvent compilation failures and
    # treat the resulting rustc action as a success, even if a metadata
    # artifact was not generated. This allows us to generate diagnostics
    # even when the target has bugs.
    diag_artifacts = {}
    clippy_artifacts = {}
    for incr in (True, False):
        diag_artifacts[incr] = rust_compile(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = Emit("metadata-fast"),
            params = strategy_params[DEFAULT_STATIC_LINK_STRATEGY],
            default_roots = default_roots,
            extra_flags = extra_flags,
            infallible_diagnostics = True,
            incremental_enabled = incr,
            transformation_spec_context = transformation_spec_context,
        )
        clippy_artifacts[incr] = rust_compile(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = Emit("clippy"),
            params = strategy_params[DEFAULT_STATIC_LINK_STRATEGY],
            default_roots = default_roots,
            extra_flags = extra_flags,
            infallible_diagnostics = True,
            incremental_enabled = incr,
            transformation_spec_context = transformation_spec_context,
        )

    providers = [
        RustcExtraOutputsInfo(
            metadata = diag_artifacts[False],
            metadata_incr = diag_artifacts[True],
            clippy = clippy_artifacts[False],
            clippy_incr = clippy_artifacts[True],
            remarks = None,  # Exposed via subtargets, not this provider
        )
    ]

    incr_enabled = ctx.attrs.incremental_enabled
    extra_compiled_targets.update(output_as_diag_subtargets(diag_artifacts[incr_enabled], clippy_artifacts[incr_enabled]))

    # Add remarks subtargets (lazy - only built when subtarget requested)
    # Uses `params` to match the actual binary's link strategy
    remarks = rust_compile(
        ctx = ctx,
        compile_ctx = compile_ctx,
        emit = bin_emit,
        params = params,
        default_roots = default_roots,
        extra_link_args = extra_link_args,
        extra_flags = extra_flags,
        incremental_enabled = False,
        profile_mode = ProfileMode("remarks"),
        transformation_spec_context = transformation_spec_context,
    )
    if remarks.compile_output.remarks_txt:
        extra_compiled_targets["remarks.txt"] = remarks.compile_output.remarks_txt
    if remarks.compile_output.remarks_json:
        extra_compiled_targets["remarks.json"] = remarks.compile_output.remarks_json

    if compile_ctx.toolchain_info.nightly_features:
        # `-Zunpretty=expanded` is unstable
        extra_compiled_targets["expand"] = rust_compile(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = Emit("expand"),
            params = strategy_params[DEFAULT_STATIC_LINK_STRATEGY],
            default_roots = default_roots,
            extra_flags = extra_flags,
            incremental_enabled = ctx.attrs.incremental_enabled,
        ).product.output

    llvm_ir_noopt = rust_compile(
        ctx = ctx,
        compile_ctx = compile_ctx,
        emit = Emit("llvm-ir-noopt"),
        params = strategy_params[DEFAULT_STATIC_LINK_STRATEGY],
        default_roots = default_roots,
        extra_flags = extra_flags,
        incremental_enabled = ctx.attrs.incremental_enabled,
    ).product.output
    if compile_ctx.toolchain_info.nightly_features:
        llvm_time_trace = rust_compile(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = bin_emit,
            params = params,
            default_roots = default_roots,
            extra_link_args = extra_link_args,
            extra_flags = extra_flags,
            incremental_enabled = ctx.attrs.incremental_enabled,
            profile_mode = ProfileMode("llvm-time-trace"),
        )
        self_profile = rust_compile(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = bin_emit,
            params = params,
            default_roots = default_roots,
            extra_link_args = extra_link_args,
            extra_flags = extra_flags,
            incremental_enabled = ctx.attrs.incremental_enabled,
            profile_mode = ProfileMode("self-profile"),
        )
    else:
        # `-Zllvm-time-trace` and `-Zself-profile` are unstable
        llvm_time_trace = None
        self_profile = None
    profiles = make_profile_providers(
        ctx = ctx,
        compile_ctx = compile_ctx,
        llvm_ir_noopt = llvm_ir_noopt,
        llvm_time_trace = llvm_time_trace,
        self_profile = self_profile,
    )
    sub_targets["profile"] = profiles

    for emit_type in ["asm", "llvm-ir", "mir"]:
        extra_compiled_targets[emit_type] = rust_compile(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = Emit(emit_type),
            params = params,
            default_roots = default_roots,
            extra_flags = extra_flags,
            incremental_enabled = ctx.attrs.incremental_enabled,
        ).product.output

    doc_output = generate_rustdoc(
        ctx = ctx,
        compile_ctx = compile_ctx,
        params = strategy_params[DEFAULT_STATIC_LINK_STRATEGY],
        default_roots = default_roots,
        document_private_items = True,
    )
    extra_compiled_targets["doc"] = doc_output

    named_deps_names = write_named_deps_names(ctx, compile_ctx)
    if named_deps_names:
        extra_compiled_targets["named_deps"] = named_deps_names

    if dwp_output:
        sub_targets["dwp"] = [
            DefaultInfo(
                default_output = dwp_output,
                other_outputs = [shlib.lib.dwp for shlib in shared_libs if shlib.lib.dwp]
                + ([executable_shlib_args.dwp_symlink_tree] if executable_shlib_args.dwp_symlink_tree else []),
            ),
        ]

    if pdb_output:
        sub_targets[PDB_SUB_TARGET] = get_pdb_providers(pdb = pdb_output, binary = final_output)

    if use_bolt and prebolt_output:
        sub_targets["prebolt"] = [DefaultInfo(default_output = prebolt_output)]

    dupmbin_toolchain = compile_ctx.cxx_toolchain_info.dumpbin_toolchain_path
    if dupmbin_toolchain:
        sub_targets[DUMPBIN_SUB_TARGET] = get_dumpbin_providers(ctx, final_output, dupmbin_toolchain)

    sub_targets.update({k: [DefaultInfo(default_output = v)] for (k, v) in extra_compiled_targets.items()})

    if dist_bundle:
        sub_targets["dist"] = [DefaultInfo(default_output = dist_bundle)]

    # Caller-built `[miri]` content, attached so sub-targets package together.
    if miri_sub_targets != None:
        sub_targets["miri"] = miri_sub_targets

    default_info = DefaultInfo(
        default_output = dist_exe if dist_bundle else final_output,
        other_outputs = runtime_files
        + (executable_shlib_args.external_debug_info + external_debug_info if compile_ctx.cxx_toolchain_info.materialize_external_debug_info else [])
        + ([dist_bundle] if dist_bundle else []),
        sub_targets = sub_targets,
    )
    providers += [
        DistInfo(
            shared_libs = shlib_info.set,
            # The bundle is load-bearing at runtime for a bundled binary: the
            # projected exe resolves its manifest / shlib tree as bundle
            # siblings, so dist consumers that materialize only
            # `nondebug_runtime_files` must get the bundle too.
            nondebug_runtime_files = runtime_files + ([dist_bundle] if dist_bundle else []),
            relocatable_resources_contents = relocatable_resources_contents,
            relocatable_resources_json = relocatable_resources_json,
        ),
    ]
    providers.append(
        rust_analyzer_provider(
            ctx = ctx,
            compile_ctx = compile_ctx,
            default_roots = default_roots,
        )
    )
    return NativeBinary(
        providers = providers,
        default_info = default_info,
        args = args,
        sub_targets = sub_targets,
    )

def _miri_interpreted_cmd(ctx: AnalysisContext, miri: cmd_args | None) -> cmd_args:
    """Miri driver command for a `miri_interpreted` target, failing without a Miri toolchain."""
    if miri == None:
        fail("`miri_interpreted = True` on `{}` requires a toolchain providing the Miri driver".format(ctx.label))
    return miri

def rust_binary_impl(ctx: AnalysisContext) -> list[Provider]:
    compile_ctx = compile_context(ctx, binary = True)
    toolchain_info = compile_ctx.toolchain_info

    default_roots = ["main.rs"]
    extra_flags = toolchain_info.rustc_binary_flags
    strategy_params = _strategy_params(ctx, compile_ctx)

    miri = _miri_direct_command(
        ctx = ctx,
        compile_ctx = compile_ctx,
        toolchain_info = toolchain_info,
        default_roots = default_roots,
        extra_flags = extra_flags,
        runtime_env = {},
        is_test = False,
        strategy_params = strategy_params,
    )

    if ctx.attrs.miri_interpreted:
        # A Miri-only binary: the target itself is the interpreted run,
        # so its own providers are the launcher, with no native link
        # and no `[miri]` sub-target.
        return _miri_launcher(ctx, compile_ctx, _miri_interpreted_cmd(ctx, miri))

    miri_sub_targets = None
    if miri != None:
        miri_sub_targets = _miri_launcher(ctx, compile_ctx, miri)

    native = _rust_binary_common(
        ctx = ctx,
        compile_ctx = compile_ctx,
        default_roots = default_roots,
        extra_flags = extra_flags,
        allow_cache_upload = cxx_attrs_get_allow_cache_upload(ctx.attrs),
        strategy_params = strategy_params,
        miri_sub_targets = miri_sub_targets,
    )

    return native.providers + [native.default_info, RunInfo(args = native.args)]

def rust_test_impl(ctx: AnalysisContext) -> list[Provider]:
    compile_ctx = compile_context(ctx, binary = True)
    toolchain_info = compile_ctx.toolchain_info

    extra_flags = toolchain_info.rustc_test_flags or []
    if ctx.attrs.framework:
        extra_flags += ["--test"]
    extra_flags = toolchain_info.rustc_binary_flags + (extra_flags or [])

    # A test may cover a binary or a library: consider both roots.
    default_roots = ctx.attrs.default_roots or ["main.rs", "lib.rs"]

    re_executors = get_re_executors_from_props(ctx)
    strategy_params = _strategy_params(ctx, compile_ctx)
    # Every test provides `[miri]`, unless `miri_interpreted` makes the target
    # itself the Miri run; omitted without a Miri toolchain.
    miri = _miri_direct_command(
        ctx = ctx,
        compile_ctx = compile_ctx,
        toolchain_info = toolchain_info,
        default_roots = default_roots,
        extra_flags = extra_flags,
        runtime_env = ctx.attrs.env | ctx.attrs.run_env,
        is_test = True,
        strategy_params = strategy_params,
    )

    if ctx.attrs.miri_interpreted:
        # A Miri test: the target itself is the interpreted run, so its
        # own providers are the launcher and test info, with no native
        # link and no `[miri]` sub-target.
        return _miri_test_sub_targets(ctx, compile_ctx, toolchain_info, _miri_interpreted_cmd(ctx, miri), re_executors)

    miri_sub_targets = None
    if miri != None:
        miri_sub_targets = _miri_test_sub_targets(ctx, compile_ctx, toolchain_info, miri, re_executors)

    native = _rust_binary_common(
        ctx = ctx,
        compile_ctx = compile_ctx,
        default_roots = default_roots,
        extra_flags = extra_flags,
        strategy_params = strategy_params,
        allow_cache_upload = False,
        miri_sub_targets = miri_sub_targets,
    )

    return _native_test_info(ctx, native.args, re_executors) + native.providers + [native.default_info]

rust_binary = prelude_rule(
    name = "rust_binary",
    impl = rust_binary_impl,
    docs = """
        A rust\\_binary() rule builds a native executable from the supplied set of Rust source files
        and dependencies.

        If you invoke a build with the `check` flavor, then Buck will invoke rustc
        to check the code (typecheck, produce warnings, etc), but won't generate an executable code.
        When applied to binaries it produces no output; for libraries it produces metadata for
        consumers of the library.

        Note: Buck is currently tested with (and therefore supports) version 1.32.0 of Rust.
    """,
    examples = """
        For more examples, check out our [integration tests](https://github.com/facebook/buck/tree/dev/test/com/facebook/buck/rust/testdata/).

        ```
        rust_binary(
          name='greet',
          srcs=[
            'greet.rs',
          ],
          deps=[
            ':greeting',
          ],
        )

        rust_library(
          name='greeting',
          srcs=[
            'greeting.rs',
          ],
          deps=[
            ':join',
          ],
        )

        rust_library(
          name='join',
          srcs=[
            'join.rs',
          ],
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        rust_common.srcs_arg()
        | rust_common.srcs_filegroup_arg()
        | rust_common.mapped_srcs_arg()
        | rust_common.edition_arg()
        | rust_common.features_arg()
        | rust_common.rustc_flags_arg()
        | rust_common.miri_flags_arg()
        | rust_common.crate(crate_type = attrs.option(attrs.string(), default = None))
        | rust_common.crate_root()
        | rust_common.env_arg()
        | miri_interpreted_attr()
        | rust_binary_attrs_group(prefix = "")
        | rust_common_attributes(is_binary = True)
        | rust_analyzer_target_kind("bin")
        | RUST_EXECUTABLE_ATTRIBUTES
        | rust_common.cxx_toolchain_arg()
        | rust_common.rust_toolchain_arg()
        | rust_common.workspaces_arg()
        | native_common.transformation_spec_arg()
        | buck.allow_cache_upload_arg()
        | {
            "build_info_as_shared_library": attrs.bool(default = True),
            "_gen_build_info": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
            "_generated_build_info_args": attrs.list(attrs.arg(), default = []),
            "_generated_build_info_data": attrs.option(attrs.source(), default = None),
            "_generated_build_info_enabled": attrs.bool(default = False),
            "_generated_build_info_mode": attrs.enum(["full", "stable"], default = "stable"),
            "_generated_build_info_spec": BUILD_INFO_ATTR,
            "_pgo_profile": attrs.option(attrs.dep(), default = None),
        }
    ),
    uses_plugins = [RustProcMacroPlugin],
    supports_incoming_transition = True,
)

rust_test = prelude_rule(
    name = "rust_test",
    impl = rust_test_impl,
    docs = """
        A rust\\_test() rule builds a Rust test native executable from the supplied set of Rust source
        files and dependencies and runs this test.

        Note: Buck is currently tested with (and therefore supports) version 1.32.0 of Rust.
    """,
    examples = """
        For more examples, check out our [integration tests](https://github.com/facebook/buck/tree/dev/test/com/facebook/buck/rust/testdata/).

        ```
        rust_test(
          name='greet',
          srcs=[
            'greet.rs',
          ],
          deps=[
            ':greeting',
          ],
        )

        rust_library(
          name='greeting',
          srcs=[
            'greeting.rs',
          ],
          deps=[
            ':join',
          ],
        )

        rust_library(
          name='join',
          srcs=[
            'join.rs',
          ],
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        buck.inject_test_env_arg()
        | rust_common.srcs_arg()
        | rust_common.srcs_filegroup_arg()
        | rust_common.mapped_srcs_arg()
        | rust_common.edition_arg()
        | rust_common.features_arg()
        | rust_common.rustc_flags_arg()
        | rust_common.miri_flags_arg()
        | rust_common.crate(crate_type = attrs.option(attrs.string(), default = None))
        | rust_common.crate_root()
        | rust_common.default_roots_arg()
        | rust_common.run_env_arg()
        | rust_common.build_and_run_env_arg()
        | rust_binary_attrs_group(prefix = "")
        | rust_common_attributes(is_binary = True)
        | rust_analyzer_target_kind("test")
        | RUST_EXECUTABLE_ATTRIBUTES
        | {
            "framework": attrs.bool(
                default = True,
                doc = """
                Use the standard test framework. If this is set to false, then the result is a normal
                executable which requires a `main()`, etc. It is still expected to accept the
                same command-line parameters and produce the same output as the test framework.
            """,
            ),
            "supports_test_execution_caching": attrs.bool(default = True),
        }
        | miri_interpreted_attr()
        | re_test_common.test_args()
        | rust_common.cxx_toolchain_arg()
        | rust_common.rust_toolchain_arg()
        | rust_common.workspaces_arg()
        | native_common.transformation_spec_arg()
        | test_common.attributes()
        | {
            "build_info_as_shared_library": attrs.bool(default = True),
            "_gen_build_info": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
            "_generated_build_info_args": attrs.list(attrs.arg(), default = []),
            "_generated_build_info_data": attrs.option(attrs.source(), default = None),
            "_generated_build_info_enabled": attrs.bool(default = False),
            "_generated_build_info_mode": attrs.enum(["full", "stable"], default = "stable"),
            "_generated_build_info_spec": BUILD_INFO_ATTR,
        }
    ),
    uses_plugins = [RustProcMacroPlugin],
    supports_incoming_transition = True,
)
