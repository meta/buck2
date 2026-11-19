# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifacts.bzl",
    "ArtifactOutputs",  # @unused Used as a type
    "unpack_artifact_map",
)
load("@prelude//:attrs_validators.bzl", "get_attrs_validation_specs", "validation_common")
load("@prelude//:paths.bzl", "paths")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load(
    "@prelude//:resources.bzl",
    "gather_resources",
    "make_resource_info",
)
load("@prelude//cxx:cxx_link_utility.bzl", "shared_libs_symlink_tree_name")
load(
    "@prelude//cxx:omnibus.bzl",
    "get_excluded",
    "get_roots",
)
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:third_party_common.bzl", "third_party_common")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load(
    "@prelude//linking:link_info.bzl",
    "LinkedObject",  # @unused Used as a type
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "LinkableRootInfo",
    "create_linkable_graph",
    "create_linkable_graph_node",
)
load("@prelude//linking:shared_libraries.bzl", "SharedLibraryInfo", "merge_shared_libraries")
load("@prelude//python:attrs.bzl", "create_manifest_for_source_dir_attr", "python_common", "typing_arg")
load(
    "@prelude//python/linking:native_python_util.bzl",
    "merge_cxx_extension_info",
    "merge_native_deps",
)
load(
    "@prelude//third-party:build.bzl",
    "create_third_party_build_root",
    "prefix_from_label",
)
load("@prelude//third-party:providers.bzl", "ThirdPartyBuild", "third_party_build_info")
load("@prelude//unix:providers.bzl", "UnixEnv", "create_unix_env_info")
load("@prelude//utils:arglike.bzl", "ArgLike")  # @unused Used as a type
load("@prelude//utils:expect.bzl", "expect")
load("@prelude//utils:utils.bzl", "from_named_set")
load(":compile.bzl", "compile_manifests")
load(":lazy_imports.bzl", "get_lazy_imports_analyzer", "run_lazy_imports_library_analyzer")
load(
    ":manifest.bzl",
    "ManifestInfo",  # @unused Used as a type
    "create_manifest_for_source_map",
)
load(":needed_coverage.bzl", "PythonNeededCoverageInfo")
load(":python.bzl", "LazyImportsCacheTSet", "NativeDepsInfoTSet", "PythonLibraryInfo", "PythonLibraryManifests", "PythonLibraryManifestsTSet")
load(":source_db.bzl", "create_python_source_db_info", "create_source_db_no_deps")
load(":toolchain.bzl", "PythonToolchainInfo")
load(":typing.bzl", "create_per_target_type_check", "create_type_check_validation")
load(":versions.bzl", "VersionedDependenciesInfo", "gather_versioned_dependencies", "resolve_versions")

def dest_prefix(label: Label, base_module: [None, str]) -> str:
    """
    Find the prefix to use for placing files inside of the python link tree

    This uses the label's package path if `base_module` is `None`, or `base_module`,
    with '.' replaced by '/', if not None. If non-empty, the returned prefix will
    end with a '/'
    """
    if base_module == None:
        prefix = label.package
    else:
        prefix = base_module.replace(".", "/")

    # Add a leading slash if we need to, but don't do that for an empty base_module
    if prefix != "":
        prefix += "/"

    return prefix

def qualify_srcs(label: Label, base_module: [None, str], srcs: dict[str, typing.Any]) -> dict[str, typing.Any]:
    """
    Fully qualify package-relative sources with the rule's base module.

    Arguments:
        label: The label for the `python_library`. Used for errors, and to construct
               the path for each source file
        base_module: If provided, the module to prefix all files from `srcs` with in
                     the eventual binary. If `None`, use the package path.
                     Usage of this is discouraged, because it makes on-disk paths
                     not match the module in execution.
        srcs: A dictionary of {relative destination path: source file}. The derived
              base module will be prepended to the destination.
    """
    prefix = dest_prefix(label, base_module)

    # Use `path.normalize` here in case items in `srcs` contains relative paths.
    return {paths.normalize(prefix + dest): src for dest, src in srcs.items()}

def create_python_needed_coverage_info(label: Label, base_module: [None, str], srcs: list[str]) -> PythonNeededCoverageInfo:
    prefix = dest_prefix(label, base_module)
    return PythonNeededCoverageInfo(
        modules = {src: prefix + src for src in srcs},
    )

def create_python_library_info(
    actions: AnalysisActions,
    label: Label,
    native_deps: NativeDepsInfoTSet,
    is_native_dep: bool,
    srcs: [ManifestInfo, None] = None,
    src_types: [ManifestInfo, None] = None,
    bytecode: [ManifestInfo, None] = None,
    default_resources: [(ManifestInfo, list[ArgLike]), None] = None,
    standalone_resources: [(ManifestInfo, list[ArgLike]), None] = None,
    outplace_resources: [(ManifestInfo, list[ArgLike]), None] = None,
    extensions: [dict[str, LinkedObject], None] = None,
    deps: list[PythonLibraryInfo] = [],
    shared_libraries: list[SharedLibraryInfo] = [],
    extension_shared_libraries: list[SharedLibraryInfo] = [],
    par_style: str | None = None,
    package_style: str | None = None,
    lazy_imports_cache: [Artifact, None] = None,
):
    """
    Create a `PythonLibraryInfo` for a set of sources and deps

    Arguments:
        label: The label for the `python_library`. Used for errors, and to construct
               the path for each source file
        srcs: A dictionary of {relative destination path: source file}.
        resources: A dictionary of {relative destination path: source file}.
        prebuilt_libraries: Prebuilt python libraries to include.
        deps: A list of `PythonLibraryInfo` objects from dependencies. These are merged
              into the resulting `PythonLibraryInfo`, as python needs all files present
              in the end
    Return:
        A fully merged `PythonLibraryInfo` provider, or fails if deps and/or srcs
        have destination paths that collide.
    """

    manifests = PythonLibraryManifests(
        label = label,
        srcs = srcs,
        src_types = src_types,
        default_resources = default_resources,
        standalone_resources = standalone_resources,
        outplace_resources = outplace_resources,
        bytecode = bytecode,
        extensions = extensions,
    )

    new_shared_libraries = merge_shared_libraries(
        actions,
        deps = shared_libraries + [dep.shared_libraries for dep in deps],
    )

    new_extension_shared_libraries = merge_shared_libraries(
        actions,
        deps = extension_shared_libraries + [dep.extension_shared_libraries for dep in deps],
    )

    dep_cache_children = [dep.lazy_imports_caches for dep in deps if dep.lazy_imports_caches != None]
    if lazy_imports_cache != None:
        lazy_imports_caches = actions.tset(LazyImportsCacheTSet, value = lazy_imports_cache, children = dep_cache_children)
    elif dep_cache_children:
        lazy_imports_caches = actions.tset(LazyImportsCacheTSet, children = dep_cache_children)
    else:
        lazy_imports_caches = None

    return PythonLibraryInfo(
        manifests = actions.tset(PythonLibraryManifestsTSet, value = manifests, children = [dep.manifests for dep in deps]),
        shared_libraries = new_shared_libraries,
        extension_shared_libraries = new_extension_shared_libraries,
        is_native_dep = is_native_dep,
        lazy_imports_caches = lazy_imports_caches,
        native_deps = native_deps,
        par_style = par_style,
        package_style = package_style,
    )

def gather_dep_libraries(raw_deps: list[Dependency], resolve_versioned_deps: bool = True) -> (list[PythonLibraryInfo], list[SharedLibraryInfo]):
    """
    Takes a list of raw dependencies, and partitions them into python_library / shared library providers.
    If resolve_versions is True, it also collects versioned_library dependencies and uses their default version. Otherwise these are skipped, and should be handled elsewhere.
    """
    deps = []
    shared_libraries = []
    for dep in raw_deps:
        if resolve_versioned_deps and VersionedDependenciesInfo in dep:
            vdeps, vshlibs = gather_dep_libraries(resolve_versions(dep[VersionedDependenciesInfo], {}))
            deps.extend(vdeps)
            shared_libraries.extend(vshlibs)

        if PythonLibraryInfo in dep:
            deps.append(dep[PythonLibraryInfo])
        elif SharedLibraryInfo in dep:
            shared_libraries.append(dep[SharedLibraryInfo])
        else:
            # Non-Python dependencies (e.g. genrule deps on third-party whls) are
            # silently ignored here; they don't provide PythonLibraryInfo or
            # SharedLibraryInfo but are valid transitive deps.
            pass
    return (deps, shared_libraries)

def _exclude_deps_from_omnibus(ctx: AnalysisContext, srcs: dict[str, Artifact]) -> bool:
    # User-specified parameter.
    if ctx.attrs.exclude_deps_from_merged_linking:
        return True

    # In some cases, Python library rules package prebuilt native extensions,
    # in which case, we can't support library merging (since we can't re-link
    # these extensions against new libraries).
    for src in srcs:
        # TODO(agallagher): Ideally, we'd prevent sources with these suffixes
        # and requires specifying them another way to make this easier to detect.
        if paths.split_extension(src)[1] in (".so", ".dll", ".pyd"):
            return True

    return False

def _attr_srcs(ctx: AnalysisContext) -> dict[str, Artifact]:
    return dict(from_named_set(ctx.attrs.srcs))

def _attr_resources(ctx: AnalysisContext) -> dict[str, Artifact | Dependency]:
    return dict(from_named_set(ctx.attrs.resources))

def py_attr_resources(ctx: AnalysisContext) -> (dict[str, ArtifactOutputs], dict[str, ArtifactOutputs], dict[str, ArtifactOutputs]):
    """
    Return the resources provided by this rule as three maps from resource name
    to ArtifactOutputs: one for the default (inplace) build, one for standalone,
    and one for outplace.

    For python_binary resources, standalone substitutes the dep with its `.par`
    artifact, while outplace substitutes with the `[outplace]` subtarget so the
    full link-tree directory is materialized alongside the bash script.
    """
    resources = _attr_resources(ctx)
    standalone_artifacts = {}
    outplace_overrides = {}
    for key, value in resources.items():
        s_resource = value
        if not isinstance(value, Artifact) and DefaultInfo in value:
            sub_targets = value[DefaultInfo].sub_targets
            if "standalone" in sub_targets:
                s_resource = sub_targets["standalone"][DefaultInfo].default_outputs[0]
            if "outplace" in sub_targets:
                outplace_di = sub_targets["outplace"][DefaultInfo]
                outplace_overrides[key] = ArtifactOutputs(
                    default_output = outplace_di.default_outputs[0],
                    nondebug_runtime_files = outplace_di.other_outputs,
                    other_outputs = outplace_di.other_outputs,
                )
        standalone_artifacts[key] = s_resource

    default_resources = unpack_artifact_map(resources)
    standalone_resources = unpack_artifact_map(standalone_artifacts)
    outplace_resources = dict(default_resources)
    outplace_resources.update(outplace_overrides)

    return default_resources, standalone_resources, outplace_resources

def py_resources_deduped(ctx: AnalysisContext, keyed_resources: list[(str, dict[str, ArtifactOutputs])]) -> list[[(ManifestInfo, list[ArgLike]), None]]:
    """
    Build one resource manifest per entry, aliasing entries whose resource
    maps are identical to a single manifest (the maps only diverge when a
    resource dep exposes style-specific subtargets). Returns one result per
    entry, None for empty maps.
    """
    built = []
    results = []
    for suffix, resources in keyed_resources:
        if not resources:
            results.append(None)
            continue
        result = None
        for seen_resources, seen_result in built:
            if resources == seen_resources:
                result = seen_result
                break
        if result == None:
            result = py_resources(ctx, resources, suffix)
            built.append((resources, result))
        results.append(result)
    return results

def _is_shared_libs_symlink_tree_of(o: Artifact, binary: Artifact) -> bool:
    # The tree is named after the linker output, which carries a suffix for
    # each post-link stage (BOLT, build info stamping, etc.) that later renames
    # the binary. See `get_cxx_post_link_suffix`.
    prefix, _, suffix = shared_libs_symlink_tree_name(binary.short_path).partition(binary.short_path)
    return o.basename.startswith(prefix + binary.short_path) and o.basename.endswith(suffix)

def py_resources(ctx: AnalysisContext, resources: dict[str, ArtifactOutputs], suffix: str = "") -> (ManifestInfo, list[ArgLike]):
    """
    Generate a manifest to wrap this rules resources.
    """
    d = {name: resource.default_output for name, resource in resources.items()}
    hidden = []
    for name, resource in resources.items():
        for o in resource.nondebug_runtime_files:
            # HACK: this is a heuristic to detect shared libs emitted from cpp_binary rules.
            if isinstance(o, Artifact) and _is_shared_libs_symlink_tree_of(o, resource.default_output):
                # Package the binary's shared libs next to the binary
                # (the path is stored in RPATH relative to the binary).
                d[paths.join(paths.dirname(name), o.basename)] = o
            elif isinstance(o, Artifact) and o.basename.endswith("#link-tree"):
                # HACK: heuristic to detect Python runtime directories from python_binary rules.
                # Package the runtime directory next to the startup script
                d[paths.join(paths.dirname(name), o.basename)] = o
            else:
                hidden.append(o)
    manifest = create_manifest_for_source_map(ctx, "resources{}".format(suffix), d)
    return manifest, dedupe(hidden)

def _src_types(srcs: dict[str, Artifact], type_stubs: dict[str, Artifact]) -> dict[str, Artifact]:
    src_types = {}

    # First, add all `.py` files.
    for name, src in srcs.items():
        _, ext = paths.split_extension(name)
        if ext == ".py" or ext == ".pyi":
            src_types[name] = src

    # Override sources which have a corresponding type stub.
    for name, src in type_stubs.items():
        base, ext = paths.split_extension(name)
        expect(ext == ".pyi", "type stubs must have `.pyi` suffix: {}", name)
        src_types.pop(base + ".py", None)
        src_types[name] = src

    return src_types

def python_library_impl(ctx: AnalysisContext) -> list[Provider]:
    # Versioned params should be intercepted and converted away via the stub.
    expect(not ctx.attrs.versioned_srcs)
    expect(not ctx.attrs.versioned_resources)

    providers = []
    sub_targets = {}

    srcs = _attr_srcs(ctx)
    qualified_srcs = qualify_srcs(ctx.label, ctx.attrs.base_module, srcs)
    default_resources_map, standalone_resources_map, outplace_resources_map = py_attr_resources(ctx)
    standalone_resources = qualify_srcs(ctx.label, ctx.attrs.base_module, standalone_resources_map)
    outplace_resources = qualify_srcs(ctx.label, ctx.attrs.base_module, outplace_resources_map)
    default_resources = qualify_srcs(ctx.label, ctx.attrs.base_module, default_resources_map)
    type_stubs = qualify_srcs(ctx.label, ctx.attrs.base_module, from_named_set(ctx.attrs.type_stubs))
    src_types = _src_types(qualified_srcs, type_stubs)

    src_manifest = create_manifest_for_source_map(ctx, "srcs", qualified_srcs) if qualified_srcs else None
    python_toolchain = ctx.attrs._python_toolchain[PythonToolchainInfo]

    # For the common all-`.py`, no-stub-override case the type manifest lists
    # exactly the srcs, so reuse that manifest instead of writing a copy.
    if src_types == qualified_srcs:
        src_type_manifest = src_manifest
    elif src_types:
        src_type_manifest = create_manifest_for_source_map(ctx, "type_stubs", src_types)
    else:
        src_type_manifest = None

    # Compile bytecode.
    bytecode = None
    if src_manifest != None and python_toolchain.pyc_compilation_enabled:
        bytecode = compile_manifests(ctx, [src_manifest])
        sub_targets["compile"] = [DefaultInfo(default_output = bytecode.artifacts[0][0])]
        sub_targets["src-manifest"] = [DefaultInfo(default_output = src_manifest.manifest, other_outputs = [a for a, _ in src_manifest.artifacts])]

    raw_deps = ctx.attrs.deps
    default_resource_manifest, standalone_resource_manifest, outplace_resource_manifest = py_resources_deduped(
        ctx,
        [
            ("", default_resources),
            ("_standalone", standalone_resources),
            ("_outplace", outplace_resources),
        ],
    )
    deps, shared_libraries = gather_dep_libraries(raw_deps, resolve_versioned_deps = False)
    providers.append(gather_versioned_dependencies(raw_deps))

    native_deps = merge_native_deps(ctx, raw_deps, shared_deps = raw_deps)

    lazy_imports_analyzer = get_lazy_imports_analyzer(ctx)
    lazy_imports_cache_output = None
    if lazy_imports_analyzer != None and getattr(ctx.attrs, "use_lifeguard_incremental", False):
        lazy_imports_cache_output = ctx.actions.declare_output("safer_lazy_imports/library-cache.bin")

    library_info = create_python_library_info(
        ctx.actions,
        ctx.label,
        srcs = src_manifest,
        src_types = src_type_manifest,
        default_resources = default_resource_manifest,
        standalone_resources = standalone_resource_manifest,
        outplace_resources = outplace_resource_manifest,
        bytecode = bytecode,
        deps = deps,
        shared_libraries = shared_libraries,
        native_deps = native_deps,
        is_native_dep = False,
        lazy_imports_cache = lazy_imports_cache_output,
    )
    providers.append(library_info)
    providers.append(
        create_unix_env_info(
            actions = ctx.actions,
            env = UnixEnv(
                label = ctx.label,
                python_libs = [library_info],
            ),
            deps = raw_deps,
        ),
    )

    # Allow third-party-build rules to depend on Python rules.
    tp_prefix = prefix_from_label(ctx.label)
    third_party_build = third_party_build_info(
        actions = ctx.actions,
        build = ThirdPartyBuild(
            prefix = tp_prefix,
            root = create_third_party_build_root(
                ctx = ctx,
                # TODO(agallagher): use constraints to get py version.
                manifests = ([("lib/python", src_manifest)] if src_manifest != None else [])
                + ([("lib/python", default_resource_manifest[0])] if default_resource_manifest != None else []),
            ),
            manifest = ctx.actions.write_json(
                "third_party_build_manifest.json",
                dict(
                    prefix = tp_prefix,
                    py_lib_paths = ["lib/python"],
                    bin_paths = [],
                    lib_paths = [],
                    runtime_lib_paths = [],
                    libs = [],
                ),
                has_content_based_path = False,
            ),
        ),
        deps = raw_deps,
    )
    providers.append(third_party_build)
    sub_targets["third-party-build"] = [
        DefaultInfo(
            default_output = third_party_build.build.root.artifact,
            sub_targets = dict(
                manifest = [DefaultInfo(default_output = third_party_build.build.manifest)],
            ),
        ),
    ]

    providers.append(create_python_needed_coverage_info(ctx.label, ctx.attrs.base_module, srcs.keys()))

    # Source DBs.
    source_db_no_deps = create_source_db_no_deps(ctx, src_types)
    sub_targets["source-db-no-deps"] = [source_db_no_deps, create_python_source_db_info(library_info.manifests)]

    # Lazy imports library cache (action scheduling — output was declared above).
    # Use qualified_srcs (.py files) not src_types (.pyi stubs override .py),
    # because the lifeguard analyzer needs the actual Python source, not type stubs.
    if lazy_imports_cache_output != None:
        lazy_srcs = qualified_srcs or {}
        lazy_db_output = ctx.actions.write_json("safer_lazy_imports/db_no_deps.json", lazy_srcs, has_content_based_path = True)
        lazy_imports_source_db = DefaultInfo(default_output = lazy_db_output, other_outputs = lazy_srcs.values())
        run_lazy_imports_library_analyzer(
            ctx,
            lazy_imports_analyzer,
            lazy_imports_cache_output,
            lazy_imports_source_db,
        )
        sub_targets["lazy-import-cache"] = [DefaultInfo(default_output = lazy_imports_cache_output)]

    # Type check
    typing_enabled = ctx.attrs.typing != None and ctx.attrs.typing != False
    type_checker = ctx.attrs.typing[RunInfo] if typing_enabled and ctx.attrs.typing != True else python_toolchain.type_checker
    pyre_validation_spec = None
    if type_checker != None:
        type_check_info = create_per_target_type_check(
            ctx,
            type_checker,
            src_type_manifest,
            deps,
            typeshed = python_toolchain.typeshed_stubs,
            py_version = ctx.attrs.py_version_for_type_checking,
            typing_enabled = typing_enabled,
            sharding_enabled = ctx.attrs.shard_typing,
        )
        sub_targets["typecheck"] = [type_check_info]

        # Build-time type check validation (separate action for sole-output constraint)
        if typing_enabled and ctx.attrs.typing_validation:
            validation_output = create_type_check_validation(ctx, type_check_info.default_outputs[0])
            pyre_validation_spec = ValidationSpec(name = "pyre", validation_result = validation_output)

    providers.append(DefaultInfo(sub_targets = sub_targets))

    # Create, augment and provide the linkable graph.
    linkable_graph = create_linkable_graph(
        ctx,
        node = create_linkable_graph_node(
            ctx,
            # Add in any potential native root targets from our first-order deps.
            roots = get_roots(raw_deps),
            # Exclude preloaded deps from omnibus linking, to prevent preloading
            # the monolithic omnibus library.
            excluded = get_excluded(
                deps = (
                    (raw_deps if _exclude_deps_from_omnibus(ctx, qualified_srcs) else [])
                    +
                    # We also need to exclude deps that can't be re-linked, via
                    # the `LinkableRootInfo` provider (i.e. `prebuilt_cxx_library_group`).
                    [d for d in raw_deps if LinkableRootInfo not in d]
                ),
            ),
        ),
        deps = raw_deps,
    )
    providers.append(linkable_graph)

    # Link info for native python
    providers.append(
        merge_cxx_extension_info(
            ctx.actions,
            raw_deps,
            shared_deps = raw_deps,
        ),
    )

    # C++ resources.
    providers.append(
        make_resource_info(
            gather_resources(
                label = ctx.label,
                deps = raw_deps,
            ),
        ),
    )

    # Attrs validators (combined with optional pyre type-check validation spec).
    attr_validation_specs = get_attrs_validation_specs(ctx)
    if pyre_validation_spec != None:
        attr_validation_specs.append(pyre_validation_spec)
    if attr_validation_specs:
        providers.append(ValidationInfo(validations = attr_validation_specs))

    return providers

python_library = prelude_rule(
    name = "python_library",
    impl = python_library_impl,
    docs = """
        A `python_library()` rule is used to group together Python
        source files and resources to be passed together in as a `dep` of other rules.
    """,
    examples = """
        Include Python source files and resource files.

        ```
        # BUCK

        # A rule that includes a single Python file.
        python_library(
          name = 'fileutil',
          srcs = ['fileutil.py'],
          deps = [
            '//third_party/python-magic:python-magic',
          ],
        )

        # A rule that uses glob() to include all Python source files in the
        # directory in which the rule is defined. The rule also specifies a
        # resource file that gets packaged with the source file.
        python_library(
          name = 'testutil',
          srcs = glob(['testutil/**/*.py']),
          resources = [
            'testdata.dat',
          ],
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        buck.labels_arg()
        | python_common.srcs_arg()
        | python_common.resources_arg()
        | python_common.base_module_arg()
        | python_common.deps_arg()
        | python_common.exclude_deps_from_merged_linking_arg()
        | third_party_common.create_third_party_build_root_attrs()
        | {
            "ignore_compile_errors": attrs.bool(default = False),
            "lazy_imports_analyzer": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
            "resources": attrs.named_set(
                attrs.one_of(attrs.dep(), attrs.source(allow_directory = True)),
                sorted = True,
                default = [],
                doc = """
                Static files to be packaged along with the Python sources.
                 These resources can be accessed at runtime using
                 the [importlib.resources](https://docs.python.org/3/library/importlib.resources.html) module
                 from the Python standard library.
            """,
            ),
            "type_stubs": attrs.named_set(attrs.source(), sorted = True, default = []),
            "use_lifeguard_incremental": attrs.bool(default = False),
            "versioned_resources": attrs.option(attrs.versioned(attrs.named_set(attrs.source(), sorted = True)), default = None),
            "versioned_srcs": attrs.option(attrs.versioned(attrs.named_set(attrs.source(), sorted = True)), default = None),
            "zip_safe": attrs.option(attrs.bool(), default = None),
            "_create_manifest_for_source_dir": create_manifest_for_source_dir_attr(),
            "_cxx_toolchain": toolchains_common.cxx(),
            "_python_internal_tools": python_common.internal_tools_arg(),
            "_python_toolchain": toolchains_common.python(),
        }
        | buck.licenses_arg()
        | buck.contacts_arg()
        | typing_arg()
        | validation_common.attrs_validators_arg()
    ),
)
