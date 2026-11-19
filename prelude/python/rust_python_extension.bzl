# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:artifact_tset.bzl", "make_artifact_tset")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//:resources.bzl", "gather_resources", "make_resource_info")
load("@prelude//cxx:cxx_toolchain_types.bzl", "LinkerType")
load("@prelude//cxx:omnibus.bzl", "create_linkable_root", "get_roots")
load("@prelude//decls:native_common.bzl", "native_common")
load("@prelude//decls:third_party_common.bzl", "third_party_common")
load("@prelude//linking:link_groups.bzl", "merge_link_group_lib_info")
load(
    "@prelude//linking:link_info.bzl",
    "Archive",
    "ArchiveLinkable",
    "LibOutputStyle",
    "LinkInfo",
    "LinkInfos",
    "LinkStrategy",
    "LinkedObject",
    "create_merged_link_info",
    "wrap_link_infos",
)
load("@prelude//linking:linkable_graph.bzl", "create_linkable_graph", "create_linkable_graph_node", "create_linkable_node")
load("@prelude//linking:linkables.bzl", "LinkableProviders")
load("@prelude//linking:shared_libraries.bzl", "merge_shared_libraries")
load("@prelude//linking:types.bzl", "Linkage")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")
load("@prelude//python:attrs.bzl", "python_common")
load("@prelude//rust:attrs.bzl", "rust_analyzer_target_kind", "rust_common", "rust_common_attributes")
load("@prelude//rust:build.bzl", "generate_rustdoc", "rust_compile", "rust_link_shared")
load(
    "@prelude//rust:build_params.bzl",
    "BuildParams",
    "CrateType",
    "Emit",
    "LinkageLang",
    "library_build_params",
)
load("@prelude//rust:context.bzl", "CompileContext", "compile_context")
load(
    "@prelude//rust:link_info.bzl",
    "RustProcMacroPlugin",
    "dfs_dedupe_by_label",
    "inherited_link_group_lib_infos",
    "inherited_linkable_graphs",
    "inherited_merged_link_infos",
    "inherited_rust_external_debug_info",
    "inherited_shared_libs",
    "resolve_deps",
)
load("@prelude//rust:named_deps.bzl", "write_named_deps_names")
load("@prelude//rust:outputs.bzl", "RustcExtraOutputsInfo", "output_as_diag_subtargets")
load("@prelude//rust:resources.bzl", "rust_attr_resources")
load("@prelude//rust:sources.bzl", "RustSources")
load("@prelude//rust/rust-analyzer:provider.bzl", "rust_analyzer_provider")
load("@prelude//utils:utils.bzl", "value_or")
load(":cxx_python_extension.bzl", "python_extension_providers")
load(":python.bzl", "PythonLibraryInfo")
load(":python_library.bzl", "dest_prefix", "qualify_srcs")

_DEFAULT_ROOTS = ["lib.rs"]

def _dep_link_strategy(ctx: AnalysisContext) -> LinkStrategy:
    """
    How the extension `.so` links its transitive deps. A shared object needs
    position-independent inputs, so a `static` request is served by the
    `static_pic` strategy (as `cxx_library` does for its own shared output).
    """
    if ctx.attrs.link_style == None or ctx.attrs.link_style == "shared":
        return LinkStrategy("shared")
    return LinkStrategy("static_pic")

def _rust_tooling_providers(ctx: AnalysisContext, compile_ctx: CompileContext, params: BuildParams, sub_targets: dict[str, list[Provider]]) -> list[Provider]:
    """
    The outputs rust developer tooling expects from every rule that compiles a
    crate (`rust_library` / `rust_binary` expose the same set): diagnostics for
    `arc rust-check` and rust-analyzer, macro expansion, rustdoc, sources.
    Populates `sub_targets` and returns the providers to add.
    """

    # `infallible_diagnostics` makes the action succeed even when the crate
    # does not compile, so diagnostics are produced for a broken target.
    diag_artifacts = {}
    clippy_artifacts = {}
    for incr in (True, False):
        diag_artifacts[incr] = rust_compile(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = Emit("metadata-fast"),
            params = params,
            default_roots = _DEFAULT_ROOTS,
            infallible_diagnostics = True,
            incremental_enabled = incr,
        )
        clippy_artifacts[incr] = rust_compile(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = Emit("clippy"),
            params = params,
            default_roots = _DEFAULT_ROOTS,
            infallible_diagnostics = True,
            incremental_enabled = incr,
        )

    incr_enabled = ctx.attrs.incremental_enabled
    outputs = output_as_diag_subtargets(diag_artifacts[incr_enabled], clippy_artifacts[incr_enabled])
    outputs["sources"] = compile_ctx.symlinked_srcs
    outputs["doc"] = generate_rustdoc(
        ctx = ctx,
        compile_ctx = compile_ctx,
        params = params,
        default_roots = _DEFAULT_ROOTS,
        document_private_items = True,
    )
    if compile_ctx.toolchain_info.nightly_features:
        # `-Zunpretty=expanded` is unstable
        outputs["expand"] = rust_compile(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = Emit("expand"),
            params = params,
            default_roots = _DEFAULT_ROOTS,
            infallible_diagnostics = True,
            incremental_enabled = incr_enabled,
        ).product.output
    named_deps_names = write_named_deps_names(ctx, compile_ctx)
    if named_deps_names:
        outputs["named_deps"] = named_deps_names
    sub_targets.update({k: [DefaultInfo(default_output = v)] for k, v in outputs.items()})

    return [
        RustcExtraOutputsInfo(
            metadata = diag_artifacts[False],
            metadata_incr = diag_artifacts[True],
            clippy = clippy_artifacts[False],
            clippy_incr = clippy_artifacts[True],
            remarks = None,
        ),
        RustSources(tset = compile_ctx.transitive_srcs),
        rust_analyzer_provider(
            ctx = ctx,
            compile_ctx = compile_ctx,
            default_roots = _DEFAULT_ROOTS,
        ),
    ]

def rust_python_extension_impl(ctx: AnalysisContext) -> list[Provider]:
    target_os = ctx.attrs._target_os_type[OsLookup].os
    library_extension = ".pyd" if target_os == Os("windows") else ".so"
    module_name = value_or(ctx.attrs.module_name, ctx.label.name)
    soname = module_name + library_extension
    base_module = dest_prefix(ctx.label, ctx.attrs.base_module)
    pyinit_symbol = ("_PyInit_" if target_os == Os("macos") else "PyInit_") + module_name

    compile_ctx = compile_context(ctx)
    cxx_toolchain = compile_ctx.cxx_toolchain_info
    linker_info = cxx_toolchain.linker_info
    advanced_unstable_linking = compile_ctx.toolchain_info.advanced_unstable_linking
    rust_lang = LinkageLang("rust") if advanced_unstable_linking else LinkageLang("native-bundled")

    rust_params = library_build_params(
        proc_macro = False,
        lib_output_style = LibOutputStyle("pic_archive"),
        lang = rust_lang,
        linker_type = linker_info.type,
        target_os_type = ctx.attrs._target_os_type[OsLookup],
    )
    rust_link = rust_compile(
        ctx = ctx,
        compile_ctx = compile_ctx,
        emit = Emit("rlib") if rust_params.crate_type == CrateType("rlib") else Emit("link"),
        params = rust_params,
        default_roots = _DEFAULT_ROOTS,
        incremental_enabled = ctx.attrs.incremental_enabled,
    )
    rust_archive = rust_link.product.output
    deps = [dep.dep for dep in resolve_deps(ctx, compile_ctx.dep_ctx)]
    python_deps = [dep for dep in deps if PythonLibraryInfo in dep]
    inherited_graphs = dfs_dedupe_by_label(inherited_linkable_graphs(ctx, compile_ctx.dep_ctx))

    # A bundled staticlib contains the Rust dependency closure. Extract only
    # referenced members so multiple extensions can share that closure.
    retention_flags = []
    bundled_debug_info = []
    if not advanced_unstable_linking:
        retention_flags = ["/EXPORT:" + pyinit_symbol] if linker_info.type == LinkerType("windows") else ["-Wl,-u," + pyinit_symbol]
        bundled_debug_info = inherited_rust_external_debug_info(
            ctx = ctx,
            dep_ctx = compile_ctx.dep_ctx,
            link_strategy = rust_params.dep_link_strategy,
        )
    archive_link_info = LinkInfo(
        linkables = [
            ArchiveLinkable(
                archive = Archive(artifact = rust_archive),
                linker_type = linker_info.type,
                link_whole = advanced_unstable_linking,
            ),
        ],
        external_debug_info = make_artifact_tset(
            actions = ctx.actions,
            label = ctx.label,
            artifacts = filter(None, [rust_link.compile_output.dwo_output_directory]),
            children = bundled_debug_info,
        ),
        pre_flags = ctx.attrs.exported_linker_flags + retention_flags,
        # Rust archives lack the MSVC /DEFAULTLIB directives emitted by C++.
        post_flags = ctx.attrs.exported_post_linker_flags + (["msvcrt.lib"] if linker_info.type == LinkerType("windows") else []),
    )

    if advanced_unstable_linking:
        extension = rust_link_shared(
            ctx = ctx,
            compile_ctx = compile_ctx,
            dep_link_style = _dep_link_strategy(ctx),
            output = soname,
            set_soname = False,
            static_lib = archive_link_info,
        )
    else:
        cdylib_params = library_build_params(
            proc_macro = False,
            lib_output_style = LibOutputStyle("shared_lib"),
            lang = LinkageLang("native-bundled"),
            linker_type = linker_info.type,
            target_os_type = ctx.attrs._target_os_type[OsLookup],
        )
        cdylib_params = BuildParams(
            crate_type = cdylib_params.crate_type,
            reloc_model = cdylib_params.reloc_model,
            dep_link_strategy = _dep_link_strategy(ctx),
            prefix = cdylib_params.prefix,
            suffix = cdylib_params.suffix,
        )
        cdylib_link = rust_compile(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emit = Emit("link"),
            params = cdylib_params,
            default_roots = _DEFAULT_ROOTS,
            incremental_enabled = ctx.attrs.incremental_enabled,
            extra_link_args = ctx.attrs.exported_linker_flags + ctx.attrs.exported_post_linker_flags,
        )
        extension = LinkedObject(
            output = cdylib_link.product.output,
            unstripped_output = cdylib_link.product.output,
            external_debug_info = make_artifact_tset(
                actions = ctx.actions,
                label = ctx.label,
                artifacts = filter(None, [cdylib_link.compile_output.dwo_output_directory]),
                children = inherited_rust_external_debug_info(
                    ctx = ctx,
                    dep_ctx = compile_ctx.dep_ctx,
                    link_strategy = cdylib_params.dep_link_strategy,
                ),
            ),
            import_library = cdylib_link.product.import_library,
            pdb = cdylib_link.product.pdb,
            dwp = cdylib_link.product.dwp_output,
        )

    archive_link_infos = LinkInfos(default = archive_link_info)
    link_infos = {
        LibOutputStyle("archive"): archive_link_infos,
        LibOutputStyle("pic_archive"): archive_link_infos,
        LibOutputStyle("shared_lib"): LinkInfos(default = LinkInfo()),
    }

    shared_output = struct(
        default = extension.output,
        other = [],
    )

    embeddable = ctx.attrs.allow_embedding

    extension_artifacts = {}
    python_module_names = {}
    if embeddable:
        if base_module != "":
            lines = ["# auto generated stub for {}\n".format(ctx.label.raw_target())]
            stub_name = module_name + ".empty_stub"
            extension_artifacts.update(
                qualify_srcs(
                    ctx.label,
                    ctx.attrs.base_module,
                    {stub_name: ctx.actions.write(stub_name, lines, has_content_based_path = False)},
                ),
            )

        python_module_names[base_module.replace("/", ".") + module_name] = pyinit_symbol

    sub_targets = {
        "rust": [DefaultInfo(default_output = rust_archive)],
        "shared": [DefaultInfo(default_output = extension.output)],
    }
    tooling_providers = _rust_tooling_providers(ctx, compile_ctx, rust_params, sub_targets)

    resource_info = make_resource_info(
        gather_resources(
            label = ctx.label,
            resources = rust_attr_resources(ctx),
            deps = deps,
        )
    )
    # Keep the label-derived root name: flat module names collide in dyld's
    # search path when different packages contain identically named modules.
    root = create_linkable_root(
        label = ctx.label,
        link_infos = wrap_link_infos(archive_link_infos, pre_flags = ctx.attrs.linker_flags),
        deps = inherited_graphs,
    )
    linkable_providers = LinkableProviders(
        link_group_lib_info = merge_link_group_lib_info(
            children = inherited_link_group_lib_infos(ctx, compile_ctx.dep_ctx),
        ),
        linkable_graph = create_linkable_graph(
            ctx = ctx,
            node = create_linkable_graph_node(
                ctx = ctx,
                linkable_node = create_linkable_node(
                    ctx = ctx,
                    deps = inherited_graphs,
                    preferred_linkage = Linkage("any"),
                    link_infos = link_infos,
                    default_soname = soname,
                ),
            ),
            deps = inherited_graphs,
        ),
        merged_link_info = create_merged_link_info(
            ctx = ctx,
            pic_behavior = cxx_toolchain.pic_behavior,
            link_infos = link_infos,
            preferred_linkage = Linkage("static"),
            deps = inherited_merged_link_infos(ctx, compile_ctx.dep_ctx),
        ),
        shared_library_info = merge_shared_libraries(
            actions = ctx.actions,
            deps = inherited_shared_libs(ctx, compile_ctx.dep_ctx),
        ),
        linkable_root_info = root,
    )
    omnibus_graph = create_linkable_graph(
        ctx = ctx,
        node = create_linkable_graph_node(ctx = ctx, roots = get_roots(python_deps)),
        deps = inherited_graphs + python_deps,
    )

    providers = tooling_providers + [resource_info, root]
    providers.extend(
        python_extension_providers(
            ctx = ctx,
            extension = extension,
            shared_output = shared_output,
            linkable_providers = linkable_providers,
            omnibus_graph = omnibus_graph,
            extension_shared_libraries = [linkable_providers.shared_library_info],
            extension_deps = deps,
            raw_deps = deps,
            python_dep_libraries = [dep[PythonLibraryInfo] for dep in python_deps],
            soname = soname,
            base_module = base_module,
            base_module_attr = ctx.attrs.base_module,
            module_name = module_name,
            type_stub = ctx.attrs.type_stub,
            embeddable = embeddable,
            extension_artifacts = extension_artifacts,
            python_module_names = python_module_names,
            sub_targets = sub_targets,
        ),
    )
    return providers

rust_python_extension = prelude_rule(
    name = "rust_python_extension",
    impl = rust_python_extension_impl,
    docs = """
        A `rust_python_extension()` rule builds a Python extension module from Rust source code
        (typically using [PyO3](https://pyo3.rs)), producing a `.so` or a Windows `.pyd`.

        The module is importable under `base_module` and `module_name` (the rule name by default).
        `link_style` defaults to `shared`; `static` and `static_pic` both use position-independent
        dependency archives. Toolchains without advanced unstable linking always bundle Rust
        dependencies, while still honoring `link_style` for native dependencies.

        Extensions may be statically embedded into a native Python binary unless `allow_embedding`
        is false. Rust initialization symbols are not renamed, so embedded modules must have distinct
        `module_name` values.

        The rule is a rust crate as far as developer tooling is concerned: it exposes the same
        `check`, `clippy.json`, `diag.json`, `expand`, `doc` and `sources` sub-targets as
        `rust_library`, and the same rust-analyzer and diagnostics providers.
    """,
    examples = """
        ```
        # A Python extension implemented as a pyo3 #[pymodule] in Rust.
        rust_python_extension(
            name = "mymodule",
            base_module = "foo.bar",
            srcs = glob(["src/**/*.rs"]),
            crate_root = "src/lib.rs",
            deps = [
                "//third-party/rust:pyo3",
            ],
        )

        python_binary(
            name = "main",
            main_module = "foo.bar.mymodule",
            deps = [":mymodule"],
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
        | rust_common.linker_flags_arg()
        | rust_common.exported_linker_flags_arg()
        | rust_common.env_arg()
        | rust_common.crate(crate_type = attrs.option(attrs.string(), default = None))
        | rust_common.crate_root()
        | native_common.link_style()
        | rust_common_attributes(is_binary = False)
        | rust_analyzer_target_kind("lib")
        | python_common.base_module_arg()
        | {
            "allow_embedding": attrs.bool(
                default = True,
                doc = """
                Whether the extension may be statically embedded into a native
                python binary (linked directly into the executable) rather than
                shipped as a separate `.so`. Set to `False` for extensions that
                must remain dynamically loaded.
            """,
            ),
            "crate_dynamic": attrs.option(attrs.dep(), default = None),
            "doctests": attrs.default_only(attrs.bool(default = False)),
            "module_name": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                Override the imported module's name (defaults to the rule name).
            """,
            ),
            "proc_macro": attrs.default_only(attrs.bool(default = False)),
            "type_stub": attrs.option(
                attrs.source(),
                default = None,
                doc = """
                Optional `.pyi` type stub file for the extension.
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
