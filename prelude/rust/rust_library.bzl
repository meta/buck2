# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactTSet",  # @unused Used as a type
    "make_artifact_tset",
)
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//:resources.bzl", "gather_resources", "make_resource_info")
load(
    "@prelude//android:android_providers.bzl",
    "merge_android_packageable_info",
)
load(
    "@prelude//cxx:linker.bzl",
    "IMPORT_LIBRARY_SUB_TARGET",
    "PDB_SUB_TARGET",
    "get_pdb_providers",
)
load(
    "@prelude//cxx:omnibus.bzl",
    "create_linkable_root",
)
load("@prelude//decls:native_common.bzl", "native_common")
load("@prelude//decls:re_test_common.bzl", "re_test_common")
load("@prelude//decls:third_party_common.bzl", "third_party_common")
load(
    "@prelude//linking:link_groups.bzl",
    "merge_link_group_lib_info",
)
load(
    "@prelude//linking:link_info.bzl",
    "Archive",
    "ArchiveLinkable",
    "LibOutputStyle",
    "LinkInfo",
    "LinkInfos",
    "LinkStrategy",
    "LinkedObject",
    "LinkerFlags",
    "MergedLinkInfo",  # @unused Used as a type
    "SharedLibLinkable",
    "create_merged_link_info",
    "get_lib_output_style",
    "get_output_styles_for_linkage",
    "set_link_info_link_whole",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "DLOPENABLE_LIBRARY_INFO_MARKER",
    "create_linkable_graph",
    "create_linkable_graph_node",
    "create_linkable_node",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",  # @unused Used as a type
    "create_shared_libraries",
    "merge_shared_libraries",
)
load("@prelude//linking:types.bzl", "Linkage")
load("@prelude//os_lookup:defs.bzl", "OsLookup")
load("@prelude//rust:attrs.bzl", "rust_analyzer_target_kind", "rust_binary_attrs_group", "rust_common", "rust_common_attributes")
load("@prelude//rust:link_info.bzl", "RustProcMacroPlugin")
load("@prelude//rust/rust-analyzer:provider.bzl", "rust_analyzer_provider")
load(
    "@prelude//tests:re_utils.bzl",
    "RemoteTestExecutorConfig",
    "get_re_executors_from_explicit_props",
)
load(
    "@prelude//third-party:build.bzl",
    "create_third_party_build_info",
)
load(
    "@prelude//third-party:providers.bzl",
    "ThirdPartyBuildInfo",  # @unused Used as a type
    "third_party_build_info",
)
load("@prelude//unix:providers.bzl", "UnixEnv", "create_unix_env_info")
load(
    ":build.bzl",
    "generate_rustdoc",
    "generate_rustdoc_coverage",
    "generate_rustdoc_test",
    "rust_compile",
    "rust_link_shared",
)
load(
    ":build_params.bzl",
    "BuildParams",  # @unused Used as a type
    "CrateType",
    "Emit",
    "LinkageLang",
    "MetadataKind",
    "ProfileMode",  # @unused Used as a type
    "binary_build_params",
    "library_build_params",
)
load(
    ":context.bzl",
    "CompileContext",  # @unused Used as a type
    "compile_context",
)
load(":dep_context.bzl", "DepCollectionContext")
load(
    ":link_info.bzl",
    "DEFAULT_STATIC_LIB_OUTPUT_STYLE",
    "DEFAULT_STATIC_LINK_STRATEGY",
    "RustExportedLinkDeps",  # @unused Used as a type
    "RustLinkInfo",
    "RustLinkStrategyInfo",
    "RustLinkableGraphs",
    "RustNativeLinkDeps",
    "RustProcMacroInfo",
    "TransitiveDeps",
    "attr_crate",
    "dfs_dedupe_by_label",
    "inherited_exported_link_deps",
    "inherited_link_group_lib_infos",
    "inherited_linkable_graphs",
    "inherited_merged_link_infos",
    "inherited_native_link_deps",
    "inherited_rust_external_debug_info",
    "inherited_shared_libs",
    "inherited_third_party_builds",
    "resolve_deps",
    "resolve_rust_deps",
    "strategy_info",
)
load(":named_deps.bzl", "write_named_deps_names")
load(
    ":outputs.bzl",
    "RustcExtraOutputsInfo",
    "RustcOutput",  # @unused Used as a type
    "output_as_diag_subtargets",
)
load(":proc_macro_alias.bzl", "rust_proc_macro_alias")
load(":profile.bzl", "make_profile_providers")
load(":resources.bzl", "rust_attr_resources")
load(":rust_toolchain.bzl", "RustToolchainInfo", "toolchain_supports_miri")
load(
    ":sources.bzl",
    "RustSources",
    "RustSourcesTSet",
)
load(":targets.bzl", "targets")

_DEFAULT_ROOTS = ["lib.rs"]

# Add provider for default output, and for each lib output style...
_SUB_TARGET_BUILD_LANG_STYLE = {
    "cdylib": (LinkageLang("native-bundled"), LibOutputStyle("shared_lib")),
    "dylib": (LinkageLang("rust"), LibOutputStyle("shared_lib")),
    # FIXME(JakobDegen): Ideally we'd use the same
    # `subtarget_for_output_style` as C++, but that uses `static-pic`
    # instead of `static_pic`. Would be nice if that were consistent
    "static": (LinkageLang("rust"), LibOutputStyle("archive")),
    "static_pic": (LinkageLang("rust"), LibOutputStyle("pic_archive")),
    "staticlib": (LinkageLang("native-bundled"), LibOutputStyle("archive")),
}

def rust_library_impl(ctx: AnalysisContext) -> list[Provider]:
    compile_ctx = compile_context(ctx)
    toolchain_info = compile_ctx.toolchain_info

    # Multiple styles and language linkages could generate the same crate types
    # (eg procmacro or using preferred_linkage), so we need to see how many
    # distinct kinds of build we actually need to deal with.
    param_lang, lang_style_param = _build_params_for_styles(ctx, compile_ctx)

    # Grab the artifacts to use for the check subtargets. Picking a good
    # `LibOutputStyle` ensures that the subtarget shares work with the main
    # build if possible
    meta_params = lang_style_param[(LinkageLang("rust"), DEFAULT_STATIC_LIB_OUTPUT_STYLE)]

    meta_fast = rust_compile(
        ctx = ctx,
        compile_ctx = compile_ctx,
        emit = Emit("metadata-fast"),
        params = meta_params,
        default_roots = _DEFAULT_ROOTS,
        incremental_enabled = ctx.attrs.incremental_enabled,
    )

    # One Miri compile from the rlib params: MIR is strategy-independent,
    # and dylib crate types cannot run under the interpreter.
    # Proc macros run natively inside the compiler and are never interpreted.
    miri_output = None
    if toolchain_supports_miri(toolchain_info):
        if meta_params.crate_type != CrateType("proc-macro"):
            miri_output = rust_compile(
                ctx = ctx,
                compile_ctx = compile_ctx,
                emit = Emit("miri"),
                params = meta_params,
                default_roots = _DEFAULT_ROOTS,
                incremental_enabled = ctx.attrs.incremental_enabled,
                extra_flags = toolchain_info.miri_rustc_flags,
            )
    # Generate the actions to build various output artifacts. Given the set of
    # parameters we need, populate maps to the linkable and metadata
    # artifacts by linkage lang.
    param_metadata_outputs = {}
    param_output = {}
    param_subtargets = {}
    for params, langs in param_lang.items():
        link = rust_compile(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = Emit("rlib") if params.crate_type == CrateType("rlib") else Emit("link"),
            params = params,
            default_roots = _DEFAULT_ROOTS,
            incremental_enabled = ctx.attrs.incremental_enabled,
        )
        param_output[params] = link

        param_subtargets.setdefault(params, {})
        if LinkageLang("rust") in langs:
            metadata_link = link
            if toolchain_info.advanced_unstable_linking and params.crate_type == CrateType("rlib"):
                # Rustc-produced staticlibs need code and metadata in one rlib.
                rlib_for_staticlib = rust_compile(
                    ctx = ctx,
                    compile_ctx = compile_ctx,
                    emit = Emit("link"),
                    params = params,
                    default_roots = _DEFAULT_ROOTS,
                    incremental_enabled = ctx.attrs.incremental_enabled,
                )
                metadata_link = rlib_for_staticlib

            if toolchain_info.nightly_features:
                # Pipelined build: dependents that need full metadata compile
                # against the `-Zno-codegen` "hollow rlib" instead of waiting
                # for this crate's codegen.
                metadata_full = rust_compile(
                    ctx = ctx,
                    compile_ctx = compile_ctx,
                    emit = Emit("metadata-full"),
                    params = params,
                    default_roots = _DEFAULT_ROOTS,
                    incremental_enabled = ctx.attrs.incremental_enabled,
                )
            else:
                # Pipelining requires the unstable `-Zno-codegen`; dependents
                # wait for the real rlib instead.
                metadata_full = link
            param_metadata_outputs[params] = {
                MetadataKind("link"): metadata_link,
                MetadataKind("full"): metadata_full,
                MetadataKind("fast"): meta_fast,
            }

            subtargets_to_add = {}
            for emit_type in ["asm", "llvm-ir", "mir"]:
                subtargets_to_add[emit_type] = rust_compile(
                    ctx = ctx,
                    compile_ctx = compile_ctx,
                    emit = Emit(emit_type),
                    params = params,
                    default_roots = _DEFAULT_ROOTS,
                    incremental_enabled = ctx.attrs.incremental_enabled,
                )
            param_subtargets[params].update(subtargets_to_add)

    # Toolchains without a Miri driver build no miri artifacts; dependents
    # requesting the kind fail loudly (see `dependency_args`).
    if miri_output != None:
        param_metadata_outputs[meta_params][MetadataKind("miri")] = miri_output

    link_infos, linked_object = _link_infos(
        ctx = ctx,
        compile_ctx = compile_ctx,
        lang_style_param = lang_style_param,
        param_artifact = param_output,
    )

    if (
        toolchain_info.advanced_unstable_linking
        and (LibOutputStyle("shared_lib") in get_output_styles_for_linkage(Linkage(ctx.attrs.preferred_linkage)))
        and not ctx.attrs.proc_macro
    ):
        if not compile_ctx.cxx_toolchain_info.linker_info.supports_shared_libraries:
            if ctx.attrs.preferred_linkage == "shared":
                fail("{}: cannot build shared library for a toolchain that does not support shared libraries".format(ctx.label))

            # Bare-metal toolchains don't support shared libraries (ld lacks -shared).
            # Provide the static archive as fallback to satisfy the linkable graph.
            link_infos[LibOutputStyle("shared_lib")] = link_infos[LibOutputStyle("pic_archive")]
        else:
            # Rely on iteration order having produced this first but that's probably ok
            linked_object = rust_link_shared(
                ctx,
                compile_ctx,
                # Unlike `cxx_library`, `link_style` is not consulted here: the deps of a Rust DSO
                # always use the shared link strategy.
                dep_link_style = LinkStrategy("shared"),
                static_lib = link_infos[LibOutputStyle("pic_archive")].default,
            )
            link_infos[LibOutputStyle("shared_lib")] = _make_shared_lib_link_infos(
                ctx,
                linked_object,
            )

    # For doctests, we need to know two things to know how to link them. The
    # first is that we need a link strategy, which affects how deps of this
    # target are handled
    if ctx.attrs.doc_link_style:
        doc_link_strategy = LinkStrategy(ctx.attrs.doc_link_style)
    else:
        # FIXME(JakobDegen): In this position, a binary would just fall back to
        # the default link style. However, we have a little bit of additional
        # information in the form of the preferred linkage that we can use to
        # make a different decision. There's nothing technically wrong with
        # that, but a comment explaining why we want to do it would be nice
        doc_link_strategy = {
            "any": LinkStrategy("shared"),
            "shared": LinkStrategy("shared"),
            "static": DEFAULT_STATIC_LINK_STRATEGY,
        }[ctx.attrs.preferred_linkage]

    # The second thing we need is a lib output style of the regular, non-doctest
    # version of this target that we want. Rustdoc does not handle this library
    # being built in a "shared" way well, so this must be a static output style.
    if ctx.attrs.doc_link_style:
        doc_output_style = {
            "shared": DEFAULT_STATIC_LIB_OUTPUT_STYLE,
            "static": LibOutputStyle("archive"),
            "static_pic": LibOutputStyle("pic_archive"),
        }[ctx.attrs.doc_link_style]
    else:
        doc_output_style = DEFAULT_STATIC_LIB_OUTPUT_STYLE
    static_library_params = lang_style_param[(LinkageLang("rust"), doc_output_style)]

    # Among {rustdoc, doctests, macro expand}, doctests are the only one which
    # cares about linkage. So whatever build params we picked for the doctests,
    # reuse them for the other two as well
    rustdoc = generate_rustdoc(
        ctx = ctx,
        compile_ctx = compile_ctx,
        params = static_library_params,
        default_roots = _DEFAULT_ROOTS,
        document_private_items = False,
    )

    if toolchain_info.nightly_features:
        rustdoc_coverage = generate_rustdoc_coverage(
            ctx = ctx,
            compile_ctx = compile_ctx,
            params = static_library_params,
            default_roots = _DEFAULT_ROOTS,
        )

        expand = rust_compile(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = Emit("expand"),
            params = static_library_params,
            default_roots = _DEFAULT_ROOTS,
            # This is needed as rustc can generate expanded sources that do not
            # fully compile, but will report an error even if it succeeds.
            # TODO(pickett): Handle this at the rustc action level, we shouldn't
            # need to pass a special arg here, expand should just work.
            infallible_diagnostics = True,
            incremental_enabled = ctx.attrs.incremental_enabled,
        ).product.output
    else:
        # `--show-coverage` and `-Zunpretty=expanded` are unstable
        rustdoc_coverage = None
        expand = None

    llvm_ir_noopt = rust_compile(
        ctx = ctx,
        compile_ctx = compile_ctx,
        emit = Emit("llvm-ir-noopt"),
        params = static_library_params,
        default_roots = _DEFAULT_ROOTS,
        incremental_enabled = ctx.attrs.incremental_enabled,
    ).product.output
    if toolchain_info.nightly_features:
        llvm_time_trace = rust_compile(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = Emit("rlib"),
            params = static_library_params,
            default_roots = _DEFAULT_ROOTS,
            incremental_enabled = ctx.attrs.incremental_enabled,
            profile_mode = ProfileMode("llvm-time-trace"),
        )
        self_profile = rust_compile(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = Emit("rlib"),
            params = static_library_params,
            default_roots = _DEFAULT_ROOTS,
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

    # If doctests=True or False is set on the individual target, respect that.
    # Otherwise look at the global setting on the toolchain.
    if ctx.attrs.doctests != None:
        doctests_enabled = ctx.attrs.doctests
    else:
        doctests_enabled = toolchain_info.doctests

    # No doctests if cross-compiling.
    #
    # I tried `cargo test --doc --target aarch64-unknown-linux-gnu` and Cargo
    # silently did not run doc tests. We could probably make this work, but it
    # seems low value, and Cargo not running them tells me we'd be very likely
    # to hit issues with this not being supported well in rustdoc.
    if toolchain_info.rustc_target_triple != None and toolchain_info.rustc_target_triple != targets.exec_triple(ctx):
        doctests_enabled = False

    if toolchain_info.nightly_features:
        rustdoc_test_metadata_kind = MetadataKind("full") if toolchain_info.advanced_unstable_linking and not ctx.attrs.proc_macro else MetadataKind("link")
        rustdoc_test_params = binary_build_params(
            proc_macro = ctx.attrs.proc_macro,
            link_strategy = doc_link_strategy,
            linker_type = compile_ctx.cxx_toolchain_info.linker_info.type,
            target_os_type = ctx.attrs._target_os_type[OsLookup],
        )
        rustdoc_test = generate_rustdoc_test(
            ctx = ctx,
            compile_ctx = compile_ctx,
            rlib = param_metadata_outputs[static_library_params][rustdoc_test_metadata_kind].product.output,
            link_infos = link_infos,
            params = rustdoc_test_params,
            default_roots = _DEFAULT_ROOTS,
        )
    else:
        # Doctests are invoked via rustdoc's unstable `--test-runtool`
        doctests_enabled = False
        rustdoc_test = None

    # infallible_diagnostics allows us to circumvent compilation failures and
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
            params = meta_params,
            default_roots = _DEFAULT_ROOTS,
            infallible_diagnostics = True,
            incremental_enabled = incr,
        )
        clippy_artifacts[incr] = rust_compile(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = Emit("clippy"),
            params = meta_params,
            default_roots = _DEFAULT_ROOTS,
            infallible_diagnostics = True,
            incremental_enabled = incr,
        )

    # Generate single remarks artifact (lazy - only built when subtarget requested)
    # Uses meta_params to share configuration with diag/clippy builds
    remarks_artifact = rust_compile(
        ctx = ctx,
        compile_ctx = compile_ctx,
        emit = Emit("rlib"),
        params = meta_params,
        default_roots = _DEFAULT_ROOTS,
        incremental_enabled = False,
        profile_mode = ProfileMode("remarks"),
    )

    incr_enabled = ctx.attrs.incremental_enabled
    re_executors = RemoteTestExecutorConfig()
    if ctx.attrs.doc_remote_execution != None:
        re_executors = get_re_executors_from_explicit_props(ctx, ctx.attrs.doc_remote_execution)

    providers = []
    providers += _default_providers(
        lang_style_param = lang_style_param,
        param_output = param_output,
        param_subtargets = param_subtargets,
        linked_object = linked_object,
        remarks_artifact = remarks_artifact,
        rustdoc = rustdoc,
        rustdoc_test = rustdoc_test,
        doctests_enabled = doctests_enabled,
        doc_supports_test_execution_caching = ctx.attrs.doc_supports_test_execution_caching,
        re_executors = re_executors,
        check_artifacts = output_as_diag_subtargets(diag_artifacts[incr_enabled], clippy_artifacts[incr_enabled]),
        expand = expand,
        sources = compile_ctx.symlinked_srcs,
        transitive_srcs = compile_ctx.transitive_srcs,
        rustdoc_coverage = rustdoc_coverage,
        named_deps_names = write_named_deps_names(ctx, compile_ctx),
        profiles = profiles,
    )
    providers += _rust_metadata_providers(
        diag_artifacts = diag_artifacts,
        clippy_artifacts = clippy_artifacts,
    )

    if ctx.attrs.proc_macro:
        # Proc macros are built the same way whatever the requested output
        # style, so there is only one set of params (see `library_build_params`)
        providers += _proc_macro_link_providers(
            ctx = ctx,
            link = param_output[meta_params],
        )
    else:
        rust_artifacts = _rust_artifacts(
            ctx = ctx,
            compile_ctx = compile_ctx,
            lang_style_param = lang_style_param,
            param_metadata_outputs = param_metadata_outputs,
        )
        if toolchain_info.advanced_unstable_linking:
            providers += _advanced_unstable_link_providers(
                ctx = ctx,
                compile_ctx = compile_ctx,
                rust_artifacts = rust_artifacts,
                link_infos = link_infos,
                linked_object = linked_object,
            )
        else:
            providers += _stable_link_providers(
                ctx = ctx,
                compile_ctx = compile_ctx,
                rust_artifacts = rust_artifacts,
                link_infos = link_infos,
                linked_object = linked_object,
            )

    deps = [dep.dep for dep in resolve_deps(ctx, compile_ctx.dep_ctx)]
    providers.append(
        make_resource_info(
            gather_resources(
                label = ctx.label,
                resources = rust_attr_resources(ctx),
                deps = deps,
            ),
        ),
    )

    providers.append(merge_android_packageable_info(ctx.label, ctx.actions, deps))

    providers.append(
        rust_analyzer_provider(
            ctx = ctx,
            compile_ctx = compile_ctx,
            default_roots = _DEFAULT_ROOTS,
        )
    )

    return providers

def _build_params_for_styles(
    ctx: AnalysisContext, compile_ctx: CompileContext
) -> (
    dict[BuildParams, list[LinkageLang]],
    dict[(LinkageLang, LibOutputStyle), BuildParams],
):
    """
    For a given rule, return two things:
    - a set of build params we need for all combinations of linkage langages and
      link styles, mapped to which languages they apply to
    - a mapping from linkage language and link style to build params

    This is needed because different combinations may end up using the same set
    of params, and we want to minimize invocations to rustc, both for
    efficiency's sake, but also to avoid duplicate objects being linked
    together.
    """

    param_lang = {}  # param -> linkage_lang
    style_param = {}  # (linkage_lang, output_style) -> param

    target_os_type = ctx.attrs._target_os_type[OsLookup]
    linker_type = compile_ctx.cxx_toolchain_info.linker_info.type
    output_styles = get_output_styles_for_linkage(Linkage(ctx.attrs.preferred_linkage))

    # Styles+lang linkage to params
    for linkage_lang in LinkageLang:
        # Skip proc_macro + non-rust combinations
        if ctx.attrs.proc_macro and linkage_lang != LinkageLang("rust"):
            continue

        for lib_output_style in output_styles:
            if (
                ctx.attrs._rust_toolchain[RustToolchainInfo].advanced_unstable_linking
                and linkage_lang == LinkageLang("rust")
                and lib_output_style == LibOutputStyle("shared_lib")
            ):
                # This gets linked directly instead of using rustc
                continue
            params = library_build_params(
                proc_macro = ctx.attrs.proc_macro,
                lib_output_style = lib_output_style,
                lang = linkage_lang,
                linker_type = linker_type,
                target_os_type = target_os_type,
            )
            if params not in param_lang:
                param_lang[params] = []
            param_lang[params] = param_lang[params] + [linkage_lang]
            style_param[(linkage_lang, lib_output_style)] = params

    return (param_lang, style_param)

def _make_shared_lib_link_infos(ctx: AnalysisContext, linked_object: LinkedObject) -> LinkInfos:
    exported_shlib = linked_object.output

    # Link against import library on Windows.
    if linked_object.import_library:
        exported_shlib = linked_object.import_library

    return LinkInfos(
        default = LinkInfo(
            linkables = [SharedLibLinkable(lib = exported_shlib)],
            pre_flags = ctx.attrs.exported_linker_flags,
            post_flags = ctx.attrs.exported_post_linker_flags,
        ),
    )

def _link_infos(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    lang_style_param: dict[(LinkageLang, LibOutputStyle), BuildParams],
    param_artifact: dict[BuildParams, RustcOutput],
) -> (dict[LibOutputStyle, LinkInfos], LinkedObject | None):
    if ctx.attrs.proc_macro:
        # Don't need any of this for proc macros
        return ({}, None)

    advanced_unstable_linking = compile_ctx.toolchain_info.advanced_unstable_linking
    lang = LinkageLang("rust") if advanced_unstable_linking else LinkageLang("native-bundled")
    linker_type = compile_ctx.cxx_toolchain_info.linker_info.type
    output_styles = get_output_styles_for_linkage(Linkage(ctx.attrs.preferred_linkage))

    link_infos = {}
    linked_object = None
    for output_style in output_styles:
        if output_style == LibOutputStyle("shared_lib") and advanced_unstable_linking:
            # Skip it here, we're going to handle it separately above
            continue
        params = lang_style_param[(lang, output_style)]
        lib = param_artifact[params]
        external_debug_infos_to_bundle = []
        if lang == LinkageLang("native-bundled"):
            # staticlibs and cdylibs are "bundled" in the sense that they are used
            # without their dependencies by the rest of the rules. This is normally
            # correct, except that the split debuginfo rustc emits for these crate
            # types is not bundled. This is arguably inconsistent behavior from
            # rustc, but in any case, it means we need to do this bundling manually
            # by collecting all the external debuginfo from dependencies
            external_debug_infos_to_bundle = inherited_rust_external_debug_info(
                ctx = ctx,
                dep_ctx = compile_ctx.dep_ctx,
                link_strategy = params.dep_link_strategy,
            )
        external_debug_info = make_artifact_tset(
            actions = ctx.actions,
            label = ctx.label,
            artifacts = filter(None, [lib.compile_output.dwo_output_directory]),
            children = external_debug_infos_to_bundle,
        )
        if output_style == LibOutputStyle("shared_lib"):
            linked_object = LinkedObject(
                output = lib.product.output,
                unstripped_output = lib.product.output,
                external_debug_info = external_debug_info,
                import_library = lib.product.import_library,
                pdb = lib.product.pdb,
                dwp = lib.product.dwp_output,
            )

            link_infos[output_style] = _make_shared_lib_link_infos(
                ctx,
                linked_object,
            )
        else:
            link_whole = ctx.attrs.link_whole or False
            link_infos[output_style] = LinkInfos(
                default = LinkInfo(
                    linkables = [
                        ArchiveLinkable(
                            archive = Archive(artifact = lib.product.output),
                            linker_type = linker_type,
                            link_whole = link_whole,
                        )
                    ],
                    external_debug_info = external_debug_info,
                    pre_flags = ctx.attrs.exported_linker_flags,
                    post_flags = ctx.attrs.exported_post_linker_flags,
                ),
                stripped = LinkInfo(
                    linkables = [
                        ArchiveLinkable(
                            archive = Archive(artifact = lib.product.stripped_output),
                            linker_type = linker_type,
                            link_whole = link_whole,
                        )
                    ],
                    pre_flags = ctx.attrs.exported_linker_flags,
                    post_flags = ctx.attrs.exported_post_linker_flags,
                ),
            )
    return (link_infos, linked_object)

def _rust_artifacts(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    lang_style_param: dict[(LinkageLang, LibOutputStyle), BuildParams],
    param_metadata_outputs: dict[BuildParams, dict[MetadataKind, RustcOutput]],
) -> dict[LinkStrategy, RustLinkStrategyInfo]:
    pic_behavior = compile_ctx.cxx_toolchain_info.pic_behavior
    preferred_linkage = Linkage(ctx.attrs.preferred_linkage)

    # The single rlib-params Miri artifact serves every strategy, like AUL
    # reuses pic_archive metadata; dylib crate types cannot be interpreted.
    miri_params = lang_style_param[(LinkageLang("rust"), DEFAULT_STATIC_LIB_OUTPUT_STYLE)]
    miri_output = param_metadata_outputs[miri_params].get(MetadataKind("miri"))
    rust_artifacts = {}
    for link_strategy in LinkStrategy:
        lib_output_style = get_lib_output_style(link_strategy, preferred_linkage, pic_behavior)
        if ctx.attrs._rust_toolchain[RustToolchainInfo].advanced_unstable_linking:
            # Unfortunately for the purpose of downstream Rust we still need metadata for this,
            # despite not having built the shared lib via rustc; we reuse the metadata from
            # the pic_archive case, which is functionally equivalent
            lib_output_style = LibOutputStyle("pic_archive")
        params = lang_style_param[(LinkageLang("rust"), lib_output_style)]
        outputs = param_metadata_outputs[params]
        if miri_output != None:
            outputs = dict(outputs)
            outputs[MetadataKind("miri")] = miri_output
        rust_artifacts[link_strategy] = _handle_rust_artifact(ctx, compile_ctx.dep_ctx, link_strategy, outputs)
    return rust_artifacts

def _handle_rust_artifact(
    ctx: AnalysisContext, dep_ctx: DepCollectionContext, link_strategy: LinkStrategy, outputs: dict[MetadataKind, RustcOutput]
) -> RustLinkStrategyInfo:
    """
    Return the RustLinkStrategyInfo for a given set of artifacts. The main consideration
    is computing the right set of dependencies.
    """
    link_output = outputs[MetadataKind("link")]
    tdeps, rust_debug_info, tprocmacrodeps = _compute_transitive_deps(ctx, dep_ctx, link_strategy)

    toolchain_info = ctx.attrs._rust_toolchain[RustToolchainInfo]
    if toolchain_info.advanced_unstable_linking:
        rust_debug_info = None
    else:
        rust_debug_info = make_artifact_tset(
            actions = ctx.actions,
            label = ctx.label,
            artifacts = filter(None, [link_output.compile_output.dwo_output_directory]),
            children = rust_debug_info,
        )
    return RustLinkStrategyInfo(
        outputs = {m: x.product.output for m, x in outputs.items()},
        singleton_tset = {m: x.product.singleton_tset for m, x in outputs.items()},
        transitive_deps = tdeps,
        transitive_proc_macro_deps = tprocmacrodeps,
        rust_debug_info = rust_debug_info,
    )

def _default_providers(
    lang_style_param: dict[(LinkageLang, LibOutputStyle), BuildParams],
    param_output: dict[BuildParams, RustcOutput],
    param_subtargets: dict[BuildParams, dict[str, RustcOutput]],
    linked_object: LinkedObject | None,
    remarks_artifact: RustcOutput,
    rustdoc: Artifact,
    rustdoc_test: cmd_args | None,
    doctests_enabled: bool,
    doc_supports_test_execution_caching: bool,
    re_executors: RemoteTestExecutorConfig,
    check_artifacts: dict[str, Artifact | None],
    expand: Artifact | None,
    sources: Artifact,
    transitive_srcs: RustSourcesTSet,
    rustdoc_coverage: Artifact | None,
    named_deps_names: Artifact | None,
    profiles: list[Provider],
) -> list[Provider]:
    targets = {}
    targets.update(check_artifacts)
    targets["sources"] = sources
    if expand != None:
        targets["expand"] = expand
    targets["doc"] = rustdoc
    if rustdoc_coverage != None:
        targets["doc-coverage"] = rustdoc_coverage
    targets["remarks.txt"] = remarks_artifact.compile_output.remarks_txt
    targets["remarks.json"] = remarks_artifact.compile_output.remarks_json
    if named_deps_names:
        targets["named_deps"] = named_deps_names

    sub_targets = {k: [DefaultInfo(default_output = v)] for (k, v) in targets.items()}
    sub_targets["profile"] = profiles
    if linked_object:
        # FIXME(JakobDegen): Should this have some debuginfo or such in `other_outputs`? Fix or
        # leave a comment explaining
        sub_targets["shared"] = [DefaultInfo(default_output = linked_object.output)]
        if linked_object.pdb:
            sub_targets[PDB_SUB_TARGET] = get_pdb_providers(pdb = linked_object.pdb, binary = linked_object.output)
        if linked_object.import_library:
            sub_targets[IMPORT_LIBRARY_SUB_TARGET] = [DefaultInfo(default_output = linked_object.import_library)]

    for name, lang_style in _SUB_TARGET_BUILD_LANG_STYLE.items():
        if lang_style not in lang_style_param:
            continue

        param = lang_style_param[lang_style]
        artifact = param_output[param]

        nested_sub_targets = {k: [DefaultInfo(default_output = v.product.output)] for k, v in param_subtargets[param].items()}
        if artifact.product.stripped_output:
            nested_sub_targets["stripped"] = [DefaultInfo(default_output = artifact.product.stripped_output)]
        dwp_output = getattr(artifact.product, "dwp_output", None)
        if dwp_output:
            nested_sub_targets["dwp"] = [DefaultInfo(default_output = dwp_output)]

        sub_targets[name] = [
            DefaultInfo(
                default_output = artifact.product.output,
                sub_targets = nested_sub_targets,
            )
        ]

    providers = []

    if rustdoc_test != None:
        rustdoc_test_info = ExternalRunnerTestInfo(
            type = "rustdoc",
            command = [rustdoc_test],
            supports_test_execution_caching = doc_supports_test_execution_caching,
            default_executor = re_executors.default_executor,
            executor_overrides = re_executors.executor_overrides,
            # Doctests always run from the project root with project-relative paths,
            # executor or not, so `re_executors.run_from_project_root` and
            # `.use_project_relative_paths` are deliberately ignored: configuring an
            # executor must not move a doctest's cwd out from under it.
            run_from_project_root = True,
            use_project_relative_paths = True,
        )

        # Always let the user run doctests via `buck2 test :crate[doc]`
        sub_targets["doc"].append(rustdoc_test_info)

        # But only run it as a part of `buck2 test :crate` if it's not disabled
        if doctests_enabled:
            providers.append(rustdoc_test_info)

    providers.append(
        DefaultInfo(
            default_output = check_artifacts["check"],
            sub_targets = sub_targets,
        )
    )

    providers.append(
        RustSources(
            tset = transitive_srcs,
        )
    )

    return providers

def _rust_metadata_providers(diag_artifacts: dict[bool, RustcOutput], clippy_artifacts: dict[bool, RustcOutput]) -> list[Provider]:
    return [
        RustcExtraOutputsInfo(
            metadata = diag_artifacts[False],
            metadata_incr = diag_artifacts[True],
            clippy = clippy_artifacts[False],
            clippy_incr = clippy_artifacts[True],
            remarks = None,  # Exposed via subtargets, not this provider
        ),
    ]

def _proc_macro_link_providers(ctx: AnalysisContext, link: RustcOutput) -> list[Provider]:
    return [
        RustProcMacroInfo(
            crate = attr_crate(ctx),
            dylib = link.product.output,
            singleton_tset = link.product.singleton_tset,
        ),
    ]

def _linker_flags(ctx: AnalysisContext) -> LinkerFlags:
    return LinkerFlags(
        flags = ctx.attrs.linker_flags,
        exported_flags = ctx.attrs.exported_linker_flags,
        exported_post_flags = ctx.attrs.exported_post_linker_flags,
    )

def _advanced_unstable_link_providers(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    rust_artifacts: dict[LinkStrategy, RustLinkStrategyInfo],
    link_infos: dict[LibOutputStyle, LinkInfos],
    linked_object: LinkedObject | None,
) -> list[Provider]:
    crate = attr_crate(ctx)
    pic_behavior = compile_ctx.cxx_toolchain_info.pic_behavior
    preferred_linkage = Linkage(ctx.attrs.preferred_linkage)

    providers = []

    dep_ctx = compile_ctx.dep_ctx

    inherited_link_infos = inherited_merged_link_infos(ctx, dep_ctx)
    inherited_shlibs = inherited_shared_libs(ctx, dep_ctx)
    inherited_graphs_tset = inherited_linkable_graphs(ctx, dep_ctx)
    inherited_exported_deps_tset = inherited_exported_link_deps(ctx, dep_ctx)
    inherited_third_party = inherited_third_party_builds(ctx, dep_ctx)

    inherited_graphs = dfs_dedupe_by_label(inherited_graphs_tset)
    inherited_exported_deps = dfs_dedupe_by_label(inherited_exported_deps_tset)

    # Native link provider.
    merged_link_info = create_merged_link_info(
        ctx,
        pic_behavior,
        link_infos,
        deps = inherited_link_infos,
        exported_deps = filter(None, [d.get(MergedLinkInfo) for d in inherited_exported_deps]),
        preferred_linkage = preferred_linkage,
    )
    providers.append(merged_link_info)

    solibs = {}

    # Add the shared library to the list of shared libs.
    shlib_name = compile_ctx.soname

    # Only add a shared library if we generated one.
    if linked_object:
        solibs[shlib_name] = linked_object

    # Native shared library provider.
    shared_libs = create_shared_libraries(ctx, solibs)
    shared_library_info = merge_shared_libraries(
        ctx.actions,
        shared_libs,
        inherited_shlibs,
    )
    providers.append(shared_library_info)

    third_party_build_info = create_third_party_build_info(
        ctx = ctx,
        shared_libs = shared_libs.libraries,
        children = inherited_third_party,
    )
    providers.append(third_party_build_info)

    linkable_graph = create_linkable_graph(
        ctx,
        node = create_linkable_graph_node(
            ctx,
            linkable_node = create_linkable_node(
                ctx = ctx,
                preferred_linkage = preferred_linkage,
                deps = inherited_graphs,
                exported_deps = inherited_exported_deps,
                link_infos = link_infos,
                shared_libs = shared_libs,
                linker_flags = _linker_flags(ctx),
                default_soname = shlib_name,
                # Link groups have a heuristic in which they assume that a
                # preferred_linkage = "static" library needs to be linked
                # into every single link group, instead of just one.
                # Applying that same heuristic to Rust seems right, but only
                # if this target actually requested that. Opt ourselves out
                # if it didn't.
                ignore_force_static_follows_dependents = preferred_linkage != Linkage("static"),
                include_in_android_mergemap = getattr(ctx.attrs, "include_in_android_merge_map_output", True),
            ),
        ),
        deps = inherited_graphs + inherited_exported_deps,
    )

    providers.append(linkable_graph)

    # Omnibus root provider.
    linkable_root = create_linkable_root(
        label = ctx.label,
        name = shlib_name,
        link_infos = LinkInfos(
            default = set_link_info_link_whole(link_infos[LibOutputStyle("pic_archive")].default),
        ),
        deps = inherited_graphs,
    )
    providers.append(linkable_root)

    # Mark libraries that support `dlopen`.
    if getattr(ctx.attrs, "supports_python_dlopen", False):
        providers.append(DLOPENABLE_LIBRARY_INFO_MARKER)

    # We never need to add anything to this provider because Rust libraries
    # cannot act as link group libs, especially given that they only support
    # auto link groups anyway
    providers.append(merge_link_group_lib_info(children = inherited_link_group_lib_infos(ctx, compile_ctx.dep_ctx)))

    # Create rust library provider.
    providers.append(
        RustLinkInfo(
            crate = crate,
            strategies = rust_artifacts,
            native_link_deps = ctx.actions.tset(
                RustNativeLinkDeps,
                children = [
                    inherited_native_link_deps(ctx, dep_ctx),
                    # Must be visited after inherited_native_link_deps in dfs order.
                    ctx.actions.tset(
                        RustNativeLinkDeps,
                        value = [(ctx.label.configured_target(), merged_link_info)],
                    ),
                ],
            ),
            exported_link_deps = inherited_exported_deps_tset,
            shared_libs = shared_library_info,
            third_party_build_info = third_party_build_info,
            linkable_graphs = ctx.actions.tset(
                RustLinkableGraphs,
                children = [
                    inherited_graphs_tset,
                    # Must be visited after inherited_graphs_tset in dfs order.
                    ctx.actions.tset(RustLinkableGraphs, value = [linkable_graph]),
                ],
            ),
        )
    )

    providers.append(
        create_unix_env_info(
            actions = ctx.actions,
            env = UnixEnv(
                label = ctx.label,
                native_libs = [shared_libs],
            ),
            deps = [dep.dep for dep in resolve_deps(ctx, compile_ctx.dep_ctx)],
        ),
    )

    return providers

def _stable_link_providers(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    rust_artifacts: dict[LinkStrategy, RustLinkStrategyInfo],
    link_infos: dict[LibOutputStyle, LinkInfos],
    linked_object: LinkedObject | None,
) -> list[Provider]:
    providers = []

    crate = attr_crate(ctx)

    native_link_deps, shared_libs, linkable_graphs, exported_link_deps, third_party_builds = _rust_link_providers(ctx, compile_ctx.dep_ctx)

    # Create rust library provider.
    rust_link_info = RustLinkInfo(
        crate = crate,
        strategies = rust_artifacts,
        native_link_deps = native_link_deps,
        exported_link_deps = exported_link_deps,
        shared_libs = shared_libs,
        third_party_build_info = third_party_build_info(
            actions = ctx.actions,
            children = third_party_builds,
        ),
        linkable_graphs = linkable_graphs,
    )

    providers.append(rust_link_info)
    providers += _native_link_providers(ctx, compile_ctx, link_infos, linked_object, rust_link_info)
    return providers

def _rust_link_providers(
    ctx: AnalysisContext, dep_ctx: DepCollectionContext
) -> (
    RustNativeLinkDeps,
    SharedLibraryInfo,
    RustLinkableGraphs,
    RustExportedLinkDeps,
    list[ThirdPartyBuildInfo],
):
    native_link_deps = inherited_native_link_deps(ctx, dep_ctx)
    inherited_shlibs = inherited_shared_libs(ctx, dep_ctx)
    inherited_graphs = inherited_linkable_graphs(ctx, dep_ctx)
    inherited_exported_deps = inherited_exported_link_deps(ctx, dep_ctx)
    inherited_third_party = inherited_third_party_builds(ctx, dep_ctx)

    shared_libs = merge_shared_libraries(
        ctx.actions,
        deps = inherited_shlibs,
    )
    return (native_link_deps, shared_libs, inherited_graphs, inherited_exported_deps, inherited_third_party)

def _native_link_providers(
    ctx: AnalysisContext,
    compile_ctx: CompileContext,
    link_infos: dict[LibOutputStyle, LinkInfos],
    linked_object: LinkedObject | None,
    rust_link_info: RustLinkInfo,
) -> list[Provider]:
    """
    Return the set of providers needed to link Rust as a dependency for native
    (ie C/C++) code, along with relevant dependencies.
    """

    # We collected transitive deps in the Rust link providers
    inherited_shlibs = [rust_link_info.shared_libs]
    inherited_third_party = rust_link_info.third_party_build_info

    inherited_link_infos = dfs_dedupe_by_label(rust_link_info.native_link_deps)
    inherited_link_graphs = dfs_dedupe_by_label(rust_link_info.linkable_graphs)
    inherited_exported_deps = dfs_dedupe_by_label(rust_link_info.exported_link_deps)

    providers = []

    preferred_linkage = Linkage(ctx.attrs.preferred_linkage)

    # Native link provider.
    providers.append(
        create_merged_link_info(
            ctx,
            compile_ctx.cxx_toolchain_info.pic_behavior,
            link_infos,
            deps = inherited_link_infos,
            exported_deps = filter(None, [d.get(MergedLinkInfo) for d in inherited_exported_deps]),
            preferred_linkage = preferred_linkage,
        )
    )

    solibs = {}

    # Add the shared library to the list of shared libs.
    shlib_name = compile_ctx.soname

    # Only add a shared library if we generated one.
    if linked_object:
        solibs[shlib_name] = linked_object

    # Native shared library provider.
    shared_libs = create_shared_libraries(ctx, solibs)
    providers.append(
        merge_shared_libraries(
            ctx.actions,
            shared_libs,
            inherited_shlibs,
        )
    )

    third_party_build_info = create_third_party_build_info(
        ctx = ctx,
        shared_libs = shared_libs.libraries,
        children = [inherited_third_party],
    )
    providers.append(third_party_build_info)

    # Omnibus root provider.
    linkable_root = create_linkable_root(
        label = ctx.label,
        name = shlib_name,
        link_infos = LinkInfos(
            default = set_link_info_link_whole(link_infos[LibOutputStyle("pic_archive")].default),
        ),
        deps = inherited_link_graphs,
    )
    providers.append(linkable_root)

    # Mark libraries that support `dlopen`.
    if getattr(ctx.attrs, "supports_python_dlopen", False):
        providers.append(DLOPENABLE_LIBRARY_INFO_MARKER)

    linkable_graph = create_linkable_graph(
        ctx,
        node = create_linkable_graph_node(
            ctx,
            linkable_node = create_linkable_node(
                ctx = ctx,
                preferred_linkage = preferred_linkage,
                deps = inherited_link_graphs,
                exported_deps = inherited_exported_deps,
                link_infos = link_infos,
                shared_libs = shared_libs,
                linker_flags = _linker_flags(ctx),
                default_soname = shlib_name,
                include_in_android_mergemap = getattr(ctx.attrs, "include_in_android_merge_map_output", True),
            ),
        ),
        deps = inherited_link_graphs + inherited_exported_deps,
    )

    providers.append(linkable_graph)

    # We never need to add anything to this provider because Rust libraries
    # cannot act as link group libs, especially given that they only support
    # auto link groups anyway
    providers.append(merge_link_group_lib_info(children = inherited_link_group_lib_infos(ctx, compile_ctx.dep_ctx)))

    providers.append(
        create_unix_env_info(
            actions = ctx.actions,
            env = UnixEnv(
                label = ctx.label,
                native_libs = [shared_libs],
            ),
            deps = inherited_exported_deps,
        ),
    )

    return providers

# Compute transitive deps. Caller decides whether this is necessary.
def _compute_transitive_deps(
    ctx: AnalysisContext, dep_ctx: DepCollectionContext, dep_link_strategy: LinkStrategy
) -> (
    dict[MetadataKind, TransitiveDeps],
    list[ArtifactTSet],
    set[TargetLabel],
):
    toolchain_info = ctx.attrs._rust_toolchain[RustToolchainInfo]
    transitive_deps = {m: [] for m in MetadataKind}
    rust_debug_info = []
    transitive_proc_macro_deps = set()

    for dep in resolve_rust_deps(ctx, dep_ctx):
        if isinstance(dep.info, RustProcMacroInfo):
            transitive_proc_macro_deps.add(dep.label.raw_target())

            # We don't want to propagate proc macros directly, and they have no transitive deps
            continue
        strategy = strategy_info(toolchain_info, dep.info, dep_link_strategy)
        for m in MetadataKind:
            # The `miri` kind is absent when the toolchain provides no
            # Miri artifacts; every other kind is always built.
            if m in strategy.singleton_tset:
                transitive_deps[m].append(strategy.singleton_tset[m])
                transitive_deps[m].append(strategy.transitive_deps[m])

        if strategy.rust_debug_info:
            rust_debug_info.append(strategy.rust_debug_info)

        transitive_proc_macro_deps.update(strategy.transitive_proc_macro_deps)

    transitive_deps = {m: ctx.actions.tset(TransitiveDeps, children = children) for m, children in transitive_deps.items()}

    return transitive_deps, rust_debug_info, transitive_proc_macro_deps

def rust_library_macro_wrapper(rust_library: typing.Callable) -> typing.Callable:
    def wrapper(proc_macro_alias_actual = None, **kwargs):
        if kwargs.get("proc_macro") == True:
            name = kwargs["name"]
            if kwargs.get("crate", None) == None and kwargs.get("crate_dynamic", None) == None:
                kwargs["crate"] = name.replace("-", "_")

            if proc_macro_alias_actual == None:
                proc_macro_alias_actual = ":_" + name

            rust_proc_macro_alias(
                name = name,
                actual_exec = proc_macro_alias_actual,
                actual_plugin = proc_macro_alias_actual,
                default_target_platform = kwargs.get("default_target_platform", None),
                visibility = kwargs.pop("visibility", []),
                _rust_toolchain = kwargs.get("_rust_toolchain", None),
            )
            kwargs["name"] = "_" + name

        rust_library(**kwargs)

    return wrapper

rust_library = prelude_rule(
    name = "rust_library",
    impl = rust_library_impl,
    docs = """
        A rust\\_library() rule builds a native library from the supplied set of Rust source files
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
        rust_library(
          name='greeting',
          srcs=[
            'greeting.rs',
          ],
          deps=[
            ':join',
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
        | rust_common.deps_arg(is_binary = False)
        | rust_common.named_deps_arg(is_binary = False)
        | rust_common.edition_arg()
        | rust_common.features_arg()
        | rust_common.rustc_flags_arg()
        |
        # linker_flags weren't supported for rust_library in Buck v1 but the
        # fbcode macros pass them anyway. They're typically empty since the
        # config-level flags don't get injected, but it doesn't hurt to accept
        # them and it simplifies the implementation of Rust rules since they
        # don't have to know whether we're building a rust_binary or a
        # rust_library.
        rust_common.linker_flags_arg()
        | rust_common.exported_linker_flags_arg()
        | rust_common.env_arg()
        | rust_common.crate(crate_type = attrs.option(attrs.string(), default = None))
        | rust_common.crate_root()
        | native_common.preferred_linkage(preferred_linkage_type = attrs.enum(Linkage.values(), default = "any"))
        | native_common.soname()
        | native_common.link_style()
        | native_common.link_whole(link_whole_type = attrs.option(attrs.bool(), default = None))
        | rust_common_attributes(is_binary = False)
        | rust_analyzer_target_kind("lib")
        | {
            "crate_dynamic": attrs.option(attrs.dep(), default = None),
            "doc_env": rust_common.env_arg()["env"],
            "doc_supports_test_execution_caching": attrs.bool(default = True),
            "doctests": attrs.option(attrs.bool(), default = None),
            "include_in_android_merge_map_output": attrs.bool(default = True),
            "proc_macro": attrs.bool(default = False),
            "supports_python_dlopen": attrs.option(attrs.bool(), default = None),
        }
        | rust_binary_attrs_group(prefix = "doc_")
        | {
            "doc_remote_execution": attrs.option(
                re_test_common.opts_for_tests_arg(),
                default = None,
                doc = """
                Remote execution properties for this library's rustdoc test. Only a
                property dict is accepted, not the name of a profile on the remote test
                execution toolchain, so that `rust_library` does not have to depend on
                that toolchain. When unset, the rustdoc test carries no executor and
                runs wherever the test framework would otherwise place it.
            """,
            ),
        }
        | rust_common.cxx_toolchain_arg()
        | rust_common.rust_toolchain_arg()
        | rust_common.workspaces_arg()
        | third_party_common.create_third_party_build_root_attrs()
    ),
    uses_plugins = [RustProcMacroPlugin],
    supports_incoming_transition = True,
)
