# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifacts.bzl",
    "ArtifactGroupInfo",
    "ArtifactOutputs",  # @unused Used as a type
)
load("@prelude//:attrs_validators.bzl", "get_attrs_validation_specs", "validation_common")
load("@prelude//:paths.bzl", "paths")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//:resources.bzl", "gather_resources")
load("@prelude//cxx:attrs.bzl", "cxx_binary_base_attrs")
load("@prelude//cxx:cxx_context.bzl", "get_opt_cxx_toolchain_info")
load(
    "@prelude//cxx:cxx_library_utility.bzl",
    "cxx_is_gnu",
)
load("@prelude//cxx:cxx_utility.bzl", "cxx_attrs_get_allow_cache_upload")
load("@prelude//decls:common.bzl", "LinkableDepType", "buck")
load("@prelude//decls:native_common.bzl", "native_common")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load(
    "@prelude//linking:link_info.bzl",
    "LinkArgs",  # @unused Used as a type
    "LinkedObject",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    _linkable_graph = "linkable_graph",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibrary",
    "to_soname",
    "traverse_shared_library_info",
)
load("@prelude//linking:strip.bzl", "strip_debug_with_gnu_debuglink")
load("@prelude//python:attrs.bzl", "python_common", "python_executable_attrs", "typing_arg")
load("@prelude//python:compute_providers.bzl", "ExecutableType", "compute_providers")
load("@prelude//python:python.bzl", "python_attr_preload_deps")
load(
    "@prelude//python/linking:link_helper.bzl",
    "LinkProviders",
    "cxx_implicit_attrs",
    "process_native_linking_rule",
    "python_implicit_attrs",
)
load(
    "@prelude//python/linking:native.bzl",
    "generate_build_info_shared_library_after_link",
    "process_native_linking",
    "python_source_build_info_invalidation_inputs",
)
load("@prelude//python/linking:native_python_util.bzl", "compute_link_strategy", "merge_native_deps")
load("@prelude//python/linking:omnibus.bzl", "process_omnibus_linking")
load("@prelude//transitions:constraint_overrides.bzl", "constraint_overrides")
load("@prelude//utils:utils.bzl", "value_or")
load(":compile.bzl", "compile_manifests")
load(
    ":interface.bzl",
    "EntryPoint",
    "EntryPointKind",
)
load(":internal_tools.bzl", "PythonInternalToolsInfo")
load(":lazy_imports.bzl", "get_lazy_imports_analyzer", "run_lazy_imports_analyzer", "run_lazy_imports_cached_analysis", "run_lazy_imports_library_analyzer")
load(":make_py_package.bzl", "PexModules", "PexProviders", "make_py_package")
load(
    ":manifest.bzl",
    "ManifestInfo",  # @unused Used as a type
    "create_manifest_for_extensions",
    "create_manifest_for_source_map",
)
load(":python.bzl", "PythonLibraryInfo", "manifests_to_interface")
load(
    ":python_library.bzl",
    "create_python_library_info",
    "gather_dep_libraries",
    "py_attr_resources",
    "py_resources_deduped",
    "qualify_srcs",
)
load(":python_runtime_bundle.bzl", "PythonRuntimeBundleInfo")
load(":source_db.bzl", "create_dbg_source_db", "create_python_source_db_info", "create_source_db_no_deps")
load(":toolchain.bzl", "NativeLinkStrategy", "PackageStyle", "PythonToolchainInfo", "get_package_style")
load(":typing.bzl", "create_per_target_type_check", "create_type_check_validation")
load(":versions.bzl", "LibraryName", "LibraryVersion", "gather_versioned_dependencies", "resolve_versions")

# We do a lot of merging extensions, so don't use O(n) type annotations
def _merge_extensions(
    # {str: ("_a", "label")}
    extensions,
    # Label
    incoming_label,
    # {str: "_a"}
    incoming_extensions,
) -> None:
    """
    Merges a incoming_extensions into `extensions`. Fails if duplicate dests exist.
    """
    for extension_name, incoming_artifact in incoming_extensions.items():
        existing = extensions.get(extension_name)
        if existing != None and existing[0] != incoming_artifact:
            existing_artifact, existing_label = existing
            error = "Duplicate extension: {}! Conflicting mappings:\n" + "{} from {}\n" + "{} from {}"
            fail(
                error.format(
                    extension_name,
                    existing_artifact,
                    existing_label,
                    incoming_artifact,
                    incoming_label,
                ),
            )
        extensions[extension_name] = (incoming_artifact, incoming_label)

def _qualify_entry_point(main: EntryPoint, base_module: str) -> EntryPoint:
    qualname = main[1]
    fqname = qualname
    if qualname.startswith("."):
        fqname = base_module + qualname
        if fqname.startswith("."):
            fqname = fqname[1:]
    return (main[0], fqname)

def python_executable(
    ctx: AnalysisContext,
    main: EntryPoint,
    srcs: dict[str, Artifact],
    default_resources: dict[str, ArtifactOutputs],
    standalone_resources: dict[str, ArtifactOutputs] | None,
    outplace_resources: dict[str, ArtifactOutputs] | None,
    compile: bool,
    allow_cache_upload: bool,
    executable_type: ExecutableType,
) -> list[Provider] | Promise:
    # Returns a three tuple: the Python binary, all its potential runtime files,
    # and a provider for its source DB.
    raw_deps = ctx.attrs.deps

    # `preload_deps` is used later to configure `LD_PRELOAD` environment variable,
    # here we make the actual libraries to appear in the distribution.
    preload_deps = python_attr_preload_deps(ctx)
    raw_deps.extend(preload_deps)

    selected_deps = resolve_versions(
        gather_versioned_dependencies(raw_deps),
        {LibraryName(value = key): LibraryVersion(value = ver) for key, ver in ctx.attrs.version_selections.items()},
    )
    raw_deps.extend(selected_deps)

    python_deps, shared_deps = gather_dep_libraries(raw_deps, resolve_versioned_deps = False)

    src_manifest = None
    bytecode_manifest = None

    python_internal_tools = ctx.attrs._python_internal_tools[PythonInternalToolsInfo]
    for artifact in python_internal_tools.runtime_library[ArtifactGroupInfo].artifacts:
        srcs[artifact.short_path] = artifact

    if srcs:
        src_manifest = create_manifest_for_source_map(ctx, "srcs", srcs)

        python_toolchain = ctx.attrs._python_toolchain[PythonToolchainInfo]
        if python_toolchain.pyc_compilation_enabled:
            bytecode_manifest = compile_manifests(ctx, [src_manifest])

    all_default_resources = {}
    all_standalone_resources = {}
    all_outplace_resources = {}
    cxx_extra_resources = {}
    for cxx_resources in gather_resources(ctx.label, deps = raw_deps).values():
        for name, resource in cxx_resources.items():
            cxx_extra_resources[paths.join("__cxx_resources__", name)] = resource
    all_default_resources.update(cxx_extra_resources)
    all_standalone_resources.update(cxx_extra_resources)
    all_outplace_resources.update(cxx_extra_resources)

    if default_resources:
        all_default_resources.update(default_resources)
    if standalone_resources:
        all_standalone_resources.update(standalone_resources)
    if outplace_resources:
        all_outplace_resources.update(outplace_resources)

    binary_default_resources, binary_standalone_resources, binary_outplace_resources = py_resources_deduped(
        ctx,
        [
            ("", all_default_resources),
            ("_standalone", all_standalone_resources),
            ("_outplace", all_outplace_resources),
        ],
    )

    library_info = create_python_library_info(
        ctx.actions,
        ctx.label,
        srcs = src_manifest,
        src_types = src_manifest,
        default_resources = binary_default_resources,
        standalone_resources = binary_standalone_resources,
        outplace_resources = binary_outplace_resources,
        bytecode = bytecode_manifest,
        deps = python_deps,
        shared_libraries = shared_deps,
        native_deps = merge_native_deps(ctx, raw_deps, shared_deps = ctx.attrs.deps + preload_deps),
        is_native_dep = False,
        par_style = ctx.attrs.par_style,
        package_style = get_package_style(ctx).value,
    )

    source_db_no_deps = create_source_db_no_deps(ctx, srcs)

    return _convert_python_library_to_executable(
        ctx,
        _qualify_entry_point(
            main,
            ctx.attrs.base_module if ctx.attrs.base_module != None else ctx.label.package.replace("/", "."),
        ),
        library_info,
        raw_deps,
        compile,
        allow_cache_upload,
        src_manifest,
        python_deps,
        source_db_no_deps,
        executable_type,
    )

def _add_executable_subtargets(
    ctx,
    exe: PexProviders,
    dbg_source_db: DefaultInfo,
    dbg_source_db_output: Artifact | None,
    library_info: PythonLibraryInfo,
    main: EntryPoint,
    source_db_no_deps: DefaultInfo,
    src_manifest: ManifestInfo | None,
    python_deps: list[PythonLibraryInfo],
) -> (PexProviders, Artifact | None):
    python_toolchain = ctx.attrs._python_toolchain[PythonToolchainInfo]
    exe = PexProviders(
        default_output = exe.default_output,
        other_outputs = exe.other_outputs,
        other_outputs_prefix = exe.other_outputs_prefix,
        hidden_resources = exe.hidden_resources,
        sub_targets = exe.sub_targets,
        run_cmd = cmd_args(python_toolchain.run_prefix, exe.run_cmd),
        dbg_source_db = dbg_source_db_output,
    )

    exe.sub_targets.update({
        "dbg-source-db": [dbg_source_db],
        "library-info": [library_info],
        "main": [DefaultInfo(default_output = ctx.actions.write_json("main.json", main, has_content_based_path = False))],
        "source-db-no-deps": [source_db_no_deps, create_python_source_db_info(library_info.manifests)],
    })

    # Type check
    typing_enabled = ctx.attrs.typing != None and ctx.attrs.typing != False
    type_checker = ctx.attrs.typing[RunInfo] if typing_enabled and ctx.attrs.typing != True else python_toolchain.type_checker
    validation_output = None

    if type_checker != None:
        type_check_info = create_per_target_type_check(
            ctx,
            type_checker,
            src_manifest,
            python_deps,
            typeshed = python_toolchain.typeshed_stubs,
            py_version = ctx.attrs.py_version_for_type_checking,
            typing_enabled = typing_enabled,
            sharding_enabled = ctx.attrs.shard_typing,
        )
        exe.sub_targets.update({"typecheck": [type_check_info]})

        if typing_enabled and ctx.attrs.typing_validation:
            validation_output = create_type_check_validation(ctx, type_check_info.default_outputs[0])

    return exe, validation_output

def _compute_pex_providers(
    ctx,
    src_manifest: ManifestInfo | None,
    python_deps: list[PythonLibraryInfo],
    source_db_no_deps: DefaultInfo,
    main: EntryPoint,
    compile: bool,
    library: PythonLibraryInfo,
    allow_cache_upload: bool,
    shared_libs: list[(SharedLibrary, str)],
    extensions: dict[str, (LinkedObject, Label)],
    link_args: list[LinkArgs],
    extra: dict[str, typing.Any],
    link_extra_artifacts: dict[str, typing.Any],
    executable_type: ExecutableType,
    linker_map_data = None,
    gc_sections_data = None,
    native_runtime_files = [],
    build_info_manifest_entries = None,
) -> list[Provider] | Promise:
    dbg_source_db_output = ctx.actions.declare_output("dbg-db.json", has_content_based_path = True)
    dbg_source_db = create_dbg_source_db(ctx, dbg_source_db_output, src_manifest, python_deps)

    extra_artifacts = {key: value for key, value in link_extra_artifacts.items()}

    link_strategy = compute_link_strategy(ctx)
    build_args = ctx.attrs.build_args
    python_toolchain = ctx.attrs._python_toolchain[PythonToolchainInfo]
    python_internal_tools = ctx.attrs._python_internal_tools[PythonInternalToolsInfo]

    if link_strategy == NativeLinkStrategy("native"):
        entry_point = "runtime/bin/{}".format(ctx.attrs.executable_name)
        build_args.append(cmd_args("--passthrough=--runtime-binary={}".format(entry_point)))

    extra_artifacts["dbg-db.json"] = dbg_source_db_output

    # Run lazy import analysis if the attribute is enabled.
    # Prefer the cache-based path (analyze_binary from per-library caches) when
    # incremental mode is on. Fall back to the monolithic path
    # (analyze against dbg-db.json) for backward compatibility.
    lazy_imports_analyzer = get_lazy_imports_analyzer(ctx)
    if getattr(ctx.attrs, "use_lifeguard_incremental", False) and lazy_imports_analyzer != None:
        lazy_import_analysis_output = ctx.actions.declare_output("safer_lazy_imports/lazy-import-analysis.json", has_content_based_path = False)
        if library.lazy_imports_caches != None:
            dep_caches = list(library.lazy_imports_caches.traverse())
        else:
            dep_caches = []

        binary_lib_cache = ctx.actions.declare_output("safer_lazy_imports/binary-library-cache.bin")
        # This first call pulls in the hidden __par__ modules
        run_lazy_imports_library_analyzer(
            ctx,
            lazy_imports_analyzer,
            binary_lib_cache,
            source_db_no_deps,
        )

        # This call builds the Lifeguard output file
        run_lazy_imports_cached_analysis(
            ctx,
            lazy_imports_analyzer,
            lazy_import_analysis_output,
            dep_caches + [binary_lib_cache],
        )
        extra_artifacts["safer_lazy_imports/lazy-import-analysis.json"] = lazy_import_analysis_output
    elif getattr(ctx.attrs, "lazy_imports_analyzer", None):
        lazy_import_analysis_output = ctx.actions.declare_output("safer_lazy_imports/lazy-import-analysis.json", has_content_based_path = False)
        run_lazy_imports_analyzer(ctx, dbg_source_db.other_outputs, lazy_import_analysis_output, dbg_source_db_output)
        extra_artifacts["safer_lazy_imports/lazy-import-analysis.json"] = lazy_import_analysis_output

    extra_artifacts["sitecustomize.py"] = python_internal_tools.default_sitecustomize

    # Add bundled runtime
    if ctx.attrs.runtime_bundle:
        bundle = ctx.attrs.runtime_bundle[PythonRuntimeBundleInfo]
        build_args.append(cmd_args("--passthrough=--python-home=runtime"))
        extra_artifacts["runtime/bin/{}".format(bundle.py_bin.basename)] = bundle.py_bin
        extra_artifacts["runtime/lib/{}".format(bundle.stdlib.basename)] = bundle.stdlib
        if bundle.libpython:
            if link_strategy != NativeLinkStrategy("native"):
                build_args.append(cmd_args("--passthrough=--preload=runtime/lib/{}".format(bundle.libpython.basename)))
            extra_artifacts["runtime/lib/{}".format(bundle.libpython.basename)] = bundle.libpython
        if link_strategy != NativeLinkStrategy("native"):
            entry_point = "runtime/bin/{}".format(bundle.py_bin.basename)
            build_args.append(cmd_args(["--passthrough=--runtime-binary={}".format(entry_point)]))
            extra_artifacts[entry_point] = bundle.py_bin
        if ctx.attrs.runtime_bundle_full:
            extra_artifacts["runtime/include/{}".format(bundle.include.basename)] = bundle.include

    # Add additional runtime libs
    for name, dep in ctx.attrs.runtime_libs.items():
        dep_info = dep.get(DefaultInfo)
        if dep_info != None:
            extra_artifacts["runtime/lib/{}".format(name)] = dep_info.default_outputs[0]

    extra_manifests = create_manifest_for_source_map(ctx, "extra_manifests", extra_artifacts)
    package_style = get_package_style(ctx)

    # Collect HIP sidecars from shared libs before stripping (strip replaces LinkedObjects).
    hip_all_debug_files = {}
    for shlib, _libdir in shared_libs:
        for arch, files in shlib.lib.hip_arch_debug_files.items():
            hip_all_debug_files.setdefault(arch, [])
            hip_all_debug_files[arch].extend(files)

    # Strip native libraries and extensions and update the .gnu_debuglink references if we are extracting
    # debug symbols from the par
    debuginfo_files = []
    debuginfos = {}
    if ctx.attrs.strip_libpar == "extract" and package_style in [PackageStyle("standalone"), PackageStyle("outplace")] and cxx_is_gnu(ctx):
        stripped_shlibs = []
        for shlib, libdir in shared_libs:
            name = paths.join(
                libdir,
                value_or(
                    shlib.soname.as_str(),
                    shlib.lib.unstripped_output.short_path,
                ),
            )
            existing = debuginfos.get(name)
            if existing == None:
                stripped, debuginfo = strip_debug_with_gnu_debuglink(
                    ctx = ctx,
                    name = name,
                    obj = shlib.lib.unstripped_output,
                )
                debuginfos[name] = (stripped, debuginfo)
            else:
                stripped, debuginfo = existing

            shlib = SharedLibrary(
                soname = shlib.soname,
                label = shlib.label,
                lib = LinkedObject(
                    output = stripped,
                    unstripped_output = shlib.lib.unstripped_output,
                    dwp = shlib.lib.dwp,
                ),
            )
            stripped_shlibs.append((shlib, libdir))
            debuginfo_files.append(((libdir, shlib, ".debuginfo"), debuginfo))
        shared_libs = stripped_shlibs
        stripped_extensions = {}
        for name, (extension, label) in extensions.items():
            stripped, debuginfo = strip_debug_with_gnu_debuglink(
                ctx = ctx,
                name = name,
                obj = extension.unstripped_output,
            )
            stripped_extensions[name] = (
                LinkedObject(
                    output = stripped,
                    unstripped_output = extension.unstripped_output,
                    dwp = extension.dwp,
                ),
                label,
            )
            debuginfo_files.append((name + ".debuginfo", debuginfo))
        extensions = stripped_extensions

    # Combine sources and extensions into a map of all modules.
    pex_modules = PexModules(
        manifests = manifests_to_interface(library.manifests),
        extra_manifests = extra_manifests,
        compile = compile,
        extensions = create_manifest_for_extensions(
            ctx,
            extensions,
            dwp = ctx.attrs.package_split_dwarf_dwp,
        )
        if extensions
        else None,
    )

    # Add GPU sidecars to debuginfo regardless of strip_libpar setting.
    for debug_artifacts in hip_all_debug_files.values():
        for debug_artifact in debug_artifacts:
            debuginfo_files.append(
                (debug_artifact.basename, debug_artifact),
            )

    # Convert preloaded deps to a set of their names to be loaded by.
    preload_labels = set([_linkable_graph(d).label for d in python_attr_preload_deps(ctx) if _linkable_graph(d)])

    # Build the PEX.
    pex = make_py_package(
        ctx = ctx,
        python_toolchain = python_toolchain,
        python_internal_tools = python_internal_tools,
        make_py_package_cmd = ctx.attrs.make_py_package[RunInfo] if ctx.attrs.make_py_package != None else None,
        package_style = package_style,
        build_args = build_args,
        pex_modules = pex_modules,
        shared_libraries = shared_libs,
        preload_labels = preload_labels,
        main = main,
        allow_cache_upload = allow_cache_upload,
        debuginfo_files = debuginfo_files,
        link_args = link_args,
        manifest_entries_overlay = build_info_manifest_entries,
    )

    pex.sub_targets.update(extra)

    # Native PARs build their executable via cxx_executable, which already emits
    # `<binary>.resources.json` and adds it (plus the referenced C++ resources)
    # to its runtime_files. process_native_linking otherwise drops these, so the
    # manifest is declared next to the binary but never materialized, and the C++
    # build::ExternalResourceManager FATALs at startup with
    # "Cannot find resource: ...". Re-attach them as PAR runtime files so they
    # land next to the executable.
    if native_runtime_files:
        pex.other_outputs.extend(native_runtime_files)

    if linker_map_data != None:
        pex.sub_targets["linker-map"] = [
            DefaultInfo(
                default_output = linker_map_data.map,
                other_outputs = [linker_map_data.binary],
            )
        ]

    if gc_sections_data != None:
        pex.sub_targets["gc-sections"] = [
            DefaultInfo(
                default_output = gc_sections_data.gc_sections,
                other_outputs = [gc_sections_data.binary],
            )
        ]

    if hip_all_debug_files:
        all_files = [f for files in hip_all_debug_files.values() for f in files]
        hip_debug_manifest = ctx.actions.write(
            "__hip_debug_manifest_par__.txt",
            cmd_args(all_files, delimiter = "\n"),
        )
        pex.sub_targets["hip_debug"] = [
            DefaultInfo(
                default_output = hip_debug_manifest,
                other_outputs = all_files,
                sub_targets = {arch: [DefaultInfo(default_outputs = files)] for arch, files in hip_all_debug_files.items()},
            ),
        ]

    updated_pex, validation_output = _add_executable_subtargets(
        ctx, pex, dbg_source_db, dbg_source_db_output, library, main, source_db_no_deps, src_manifest, python_deps
    )

    providers = compute_providers(ctx, updated_pex, executable_type)

    # Build-time type check validation
    validation_specs = get_attrs_validation_specs(ctx)
    if validation_output != None:
        validation_specs.append(ValidationSpec(name = "pyre", validation_result = validation_output))
    if validation_specs:
        providers.append(ValidationInfo(validations = validation_specs))

    return providers

def _replace_placeholder_build_info(
    ctx: AnalysisContext,
    shared_libs: list[(SharedLibrary, str)],
    extra: dict[str, typing.Any],
    link_args: list[LinkArgs],
    build_info_manifest_entries: Artifact | None,
    generated_build_info_invalidation_deps: list[Dependency],
    generated_build_info_invalidation_sources: list[Artifact],
    output_suffix: str = "",
) -> (list[(SharedLibrary, str)], dict[str, typing.Any], Artifact | None):
    """Generate the real full-mode build-info DSO and swap it in.

    The native link (anon or not) linked a stable-mode placeholder DSO, so
    its actions are byte-stable across build hosts. This regenerates the
    real DSO in the `python_binary` analysis and replaces the placeholder
    in the packaged shared libraries. No-op unless
    `_generate_build_info_after_link` is set.
    """
    if not ctx.attrs._generate_build_info_after_link:
        return (shared_libs, extra, build_info_manifest_entries)

    invalidation_inputs = python_source_build_info_invalidation_inputs(
        ctx,
        generated_build_info_invalidation_deps,
        generated_build_info_invalidation_sources,
    )
    if "native-executable" in extra:
        # The linked binary invalidates the real build info, mirroring the
        # invalidation the link-internal generation used.
        invalidation_inputs = invalidation_inputs + [
            extra["native-executable"][0].default_outputs[0],
        ]
    after_link = generate_build_info_shared_library_after_link(
        ctx,
        shared_libs,
        extra,
        link_args,
        invalidation_inputs,
        output_suffix = output_suffix,
    )
    if after_link == None:
        return (shared_libs, extra, build_info_manifest_entries)

    shared_libs, extra, real_build_info = after_link
    build_info_manifest_entries = real_build_info.manifest_entries
    return (shared_libs, extra, build_info_manifest_entries)

def _compute_pex_providers_from_anon(
    ctx: AnalysisContext,
    providers,
    src_manifest: ManifestInfo | None,
    python_deps: list[PythonLibraryInfo],
    source_db_no_deps: DefaultInfo,
    main: EntryPoint,
    compile: bool,
    library: PythonLibraryInfo,
    allow_cache_upload: bool,
    executable_type: ExecutableType,
    generated_build_info_invalidation_deps: list[Dependency],
    generated_build_info_invalidation_sources: list[Artifact],
) -> list[Provider]:
    """`_compute_pex_providers` for the anon-target native-linking path.

    The anon target linked a stable-mode placeholder build-info DSO (via the
    the anon rule links a stable-mode placeholder DSO in full mode).
    Generate the real full-mode DSO here and swap it in, so the packaged
    artifacts carry the real per-build values while every anon action stays
    byte-stable across build hosts.
    """
    shared_libs, extra, build_info_manifest_entries = _replace_placeholder_build_info(
        ctx,
        providers[LinkProviders].shared_libraries,
        providers[LinkProviders].extra,
        providers[LinkProviders].link_args,
        providers[LinkProviders].build_info_manifest_entries,
        generated_build_info_invalidation_deps,
        generated_build_info_invalidation_sources,
    )

    return _compute_pex_providers(
        ctx,
        src_manifest,
        python_deps,
        source_db_no_deps,
        main,
        compile,
        library,
        allow_cache_upload,
        shared_libs,
        providers[LinkProviders].extensions,
        providers[LinkProviders].link_args,
        extra,
        providers[LinkProviders].extra_artifacts,
        executable_type,
        providers[LinkProviders].linker_map_data,
        providers[LinkProviders].gc_sections_data,
        native_runtime_files = providers[LinkProviders].runtime_files,
        build_info_manifest_entries = build_info_manifest_entries,
    )

def _convert_python_library_to_executable(
    ctx: AnalysisContext,
    main: EntryPoint,
    library: PythonLibraryInfo,
    deps: list[Dependency],
    compile: bool,
    allow_cache_upload: bool,
    src_manifest: ManifestInfo | None,
    python_deps: list[PythonLibraryInfo],
    source_db_no_deps: DefaultInfo,
    executable_type: ExecutableType,
) -> list[Provider] | Promise:
    extra = {}

    python_toolchain = ctx.attrs._python_toolchain[PythonToolchainInfo]
    python_internal_tools = ctx.attrs._python_internal_tools[PythonInternalToolsInfo]
    package_style = get_package_style(ctx)

    extra_artifacts = {}
    link_args = []
    link_strategy = compute_link_strategy(ctx)
    native_runtime_files = []

    if link_strategy == NativeLinkStrategy("native"):
        invalidate_build_info_on_python_sources = getattr(ctx.attrs, "_generated_build_info_enabled", False) and ctx.attrs._generated_build_info_mode == "full"
        generated_build_info_invalidation_deps = deps if invalidate_build_info_on_python_sources else []
        generated_build_info_invalidation_sources = (
            [artifact for artifact, _ in src_manifest.artifacts] if invalidate_build_info_on_python_sources and src_manifest != None else []
        )

        use_anon_target = getattr(ctx.attrs, "use_anon_target_for_analysis", False)
        if use_anon_target:
            native_deps = {}
            dlopen_deps = {}
            shared_only_deps = {}
            for info in library.native_deps.traverse():
                native_deps.update(info.native_deps)
                dlopen_deps.update(info.dlopen_deps)
                shared_only_deps.update(info.shared_only_deps)

            explicit_attrs = {
                "allow_cache_upload": allow_cache_upload,
                "deps": list(native_deps.values()),
                "dlopen_deps": list(dlopen_deps.values()),
                "generated_build_info_invalidation_deps": generated_build_info_invalidation_deps,
                "generated_build_info_invalidation_sources": generated_build_info_invalidation_sources,
                "name": "python_linking:" + ctx.attrs.name,
                "package_style": package_style.value,
                "rpath": ctx.attrs.name,
                "shared_only_deps": list(shared_only_deps.values()),
                "static_extension_utils": ctx.attrs.static_extension_utils,
                "transformation_spec": ctx.attrs.transformation_spec,
                "_cxx_toolchain": ctx.attrs._cxx_toolchain,
                "_generate_build_info_after_link": ctx.attrs._generate_build_info_after_link,
                "_python_internal_tools": ctx.attrs._python_internal_tools,
                "_python_toolchain": ctx.attrs._python_toolchain,
            }
            implicit_attrs = {
                a: getattr(ctx.attrs, a) for a in (set(cxx_implicit_attrs.keys()) | set(python_implicit_attrs.keys())) - set(explicit_attrs.keys())
            }
            return ctx.actions.anon_target(
                process_native_linking_rule,
                explicit_attrs | implicit_attrs,
            ).promise.map(
                lambda providers: _compute_pex_providers_from_anon(
                    ctx,
                    providers,
                    src_manifest,
                    python_deps,
                    source_db_no_deps,
                    main,
                    compile,
                    library,
                    allow_cache_upload,
                    executable_type,
                    generated_build_info_invalidation_deps,
                    generated_build_info_invalidation_sources,
                )
            )
        else:
            shared_libs, extensions, link_args, extra, extra_artifacts, linker_map_data, gc_sections_data, native_runtime_files, build_info_manifest_entries = (
                process_native_linking(
                    ctx,
                    deps,
                    python_toolchain,
                    python_internal_tools,
                    package_style,
                    allow_cache_upload,
                    generated_build_info_invalidation_deps = generated_build_info_invalidation_deps,
                    generated_build_info_invalidation_sources = generated_build_info_invalidation_sources,
                )
            )
            shared_libs, extra, build_info_manifest_entries = _replace_placeholder_build_info(
                ctx,
                shared_libs,
                extra,
                link_args,
                build_info_manifest_entries,
                generated_build_info_invalidation_deps,
                generated_build_info_invalidation_sources,
                # Same-context regeneration: keep its outputs distinct from
                # the placeholder generation inside the link.
                output_suffix = "-real",
            )
            if ctx.attrs.runtime_bundle:
                runtime_bundle = ctx.attrs.runtime_bundle[PythonRuntimeBundleInfo]

                # On some platforms libpython does not link with system libraries. Include libraries
                # in the PAR in this case.
                for dep in runtime_bundle.shared_libs:
                    lib = dep.get(DefaultInfo).default_outputs[0]
                    shared_libs.append(
                        (
                            # There's probably a smarter way to get the shared library object out of the
                            # dependency, but I'm not sure what that is.
                            SharedLibrary(
                                soname = to_soname(lib.basename),
                                label = dep.label,
                                lib = LinkedObject(output = lib, unstripped_output = lib),
                            ),
                            "",
                        )
                    )

    else:
        build_info_manifest_entries = None
        linker_map_data = None
        gc_sections_data = None
        extensions = {}
        for manifest in library.manifests.traverse():
            if manifest.extensions:
                _merge_extensions(extensions, manifest.label, manifest.extensions)
        if link_strategy == NativeLinkStrategy("merged"):
            shared_libs, extensions = process_omnibus_linking(ctx, deps, extensions, python_toolchain, extra)
        else:
            shared_libs = [(shared_lib, "") for shared_lib in traverse_shared_library_info(library.shared_libraries, transformation_provider = None)]

            # darwin and windows expect self-contained dynamically linked
            # python extensions without additional transitive shared libraries
            shared_libs += [
                (extension_shared_lib, "")
                for extension_shared_lib in traverse_shared_library_info(library.extension_shared_libraries, transformation_provider = None)
            ]

    return _compute_pex_providers(
        ctx,
        src_manifest,
        python_deps,
        source_db_no_deps,
        main,
        compile,
        library,
        allow_cache_upload,
        shared_libs,
        extensions,
        link_args,
        extra,
        extra_artifacts,
        executable_type,
        linker_map_data = linker_map_data if link_strategy == NativeLinkStrategy("native") else None,
        gc_sections_data = gc_sections_data if link_strategy == NativeLinkStrategy("native") else None,
        native_runtime_files = native_runtime_files,
        build_info_manifest_entries = build_info_manifest_entries,
    )

def python_binary_impl(ctx: AnalysisContext) -> list[Provider] | Promise:
    main_module = ctx.attrs.main_module
    main_function = ctx.attrs.main_function
    if main_module != None and ctx.attrs.main != None:
        fail("Only one of main_module or main may be set. Prefer main_function as main and main_module are considered deprecated")
    elif main_module != None and main_function != None:
        fail("Only one of main_module or main_function may be set. Prefer main_function.")
    elif ctx.attrs.main != None and main_function == None:
        main_module = "." + ctx.attrs.main.short_path.replace("/", ".")
        if main_module.endswith(".py"):
            main_module = main_module[:-3]

    if main_module != None:
        main = (EntryPointKind("module"), main_module)
    else:
        main = (EntryPointKind("function"), main_function)

    srcs = {}
    if ctx.attrs.main != None:
        srcs[ctx.attrs.main.short_path] = ctx.attrs.main
    srcs = qualify_srcs(ctx.label, ctx.attrs.base_module, srcs)
    default_resources_map, standalone_resources_map, outplace_resources_map = py_attr_resources(ctx)
    standalone_resources = qualify_srcs(ctx.label, ctx.attrs.base_module, standalone_resources_map)
    outplace_resources = qualify_srcs(ctx.label, ctx.attrs.base_module, outplace_resources_map)
    default_resources = qualify_srcs(ctx.label, ctx.attrs.base_module, default_resources_map)

    cxx_toolchain_info = get_opt_cxx_toolchain_info(ctx)
    toolchain_allow_cache_upload = cxx_toolchain_info.cxx_compiler_info.allow_cache_upload if cxx_toolchain_info else None

    return python_executable(
        ctx,
        main,
        srcs,
        default_resources,
        standalone_resources,
        outplace_resources,
        compile = value_or(ctx.attrs.compile, False),
        allow_cache_upload = cxx_attrs_get_allow_cache_upload(ctx.attrs, toolchain_allow_cache_upload),
        executable_type = ExecutableType("binary"),
    )

python_binary = prelude_rule(
    name = "python_binary",
    impl = python_binary_impl,
    docs = """
        A `python_binary()` rule is used to build an executable Python package
        that includes Python sources and resources from all transitive
        dependencies.
    """,
    examples = """
        Build an executable from the Python files in the BUCK directory.

        ```
        # BUCK

        python_binary(
          name = 'tailer',
          main_module = 'tailer',
          deps = [
            ':tailerutils',
          ],
        )

        python_library(
          name = 'tailerutils',
          # The main module, tailer.py, is specified here.
          # (Separated out from the glob pattern for clarity.)
          srcs = glob(['tailer.py', '*.py']),
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {k: attrs.default_only(v) for k, v in cxx_binary_base_attrs().items()}
        | buck.labels_arg()
        | {
            "base_module": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                The package in which the main module should reside in its final
                 location in the binary. If unset, Buck uses the project-relative directory
                 that contains the BUCK file.
            """,
            ),
            "main": attrs.option(
                attrs.source(),
                default = None,
                doc = """
                The Python file which serves as the entry point for the binary.
                 The interpreter initiates execution of the binary with the code in this file.
            """,
            ),
            "main_module": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                The python module that should be the entry point of the binary. This should be
                 a module name within a `python_library` that this binary depends on. Note that
                 module names take `base_module` of the library into account.
                 This property is mutually exclusive with `main`, and should be preferred to `main`, which is deprecated.
            """,
            ),
        }
        | python_common.deps_arg()
        | python_common.version_selections_arg()
        | python_common.preload_deps_arg()
        | python_common.package_style_arg()
        | python_common.linker_flags_arg()
        | python_common.deduplicate_merged_link_roots()
        | python_common.executable_deps_arg()
        | native_common.link_group_deps()
        | native_common.transformation_spec_arg()
        | native_common.link_group_public_deps_label()
        | {
            "build_args": attrs.list(attrs.arg(), default = []),
            "compile": attrs.option(attrs.bool(), default = None),
            "dummy_omnibus": attrs.option(attrs.dep(), default = None),
            "extension": attrs.option(attrs.string(), default = None),
            "prefer_stripped_native_objects": attrs.bool(default = False),
            "repl_main": attrs.option(attrs.string(), default = None),
            "repl_only_deps": attrs.list(attrs.dep(), default = []),
            "zip_safe": attrs.option(attrs.bool(), default = None),
        }
        | buck.licenses_arg()
        | buck.contacts_arg()
        | buck.allow_cache_upload_arg()
        | typing_arg()
        | python_executable_attrs()
        | {
            "link_style": attrs.enum(LinkableDepType, default = "static"),
            "_python_internal_tools": python_common.internal_tools_arg(),
            "_python_toolchain": toolchains_common.python(),
        }
        | validation_common.attrs_validators_arg()
    ),
    cfg = constraint_overrides.python_transition,
)
