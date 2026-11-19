# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load("@prelude//cxx:cxx_toolchain_types.bzl", "PicBehavior")
load(
    "@prelude//cxx:link.bzl",
    "cxx_link_shared_library",
)
load(
    "@prelude//cxx:link_types.bzl",
    "link_options",
)
load(
    "@prelude//cxx:omnibus.bzl",
    "create_omnibus_libraries",
    "get_omnibus_graph",
    "get_roots",
)
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//linking:execution_preference.bzl", "LinkExecutionPreference", "link_execution_preference_attr")
load(
    "@prelude//linking:link_info.bzl",
    "LibOutputStyle",
    "LinkArgs",
    "LinkStrategy",
    "get_lib_output_style",
    "get_link_info",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "LinkableGraph",
    "LinkableNode",  # @unused Used as a type
    "LinkableRootInfo",
    "create_linkable_graph",
    "get_deps_for_link",
    "get_linkable_graph_node_map_func",
    get_link_info_for_node = "get_link_info",
)
load("@prelude//linking:shared_libraries.bzl", "SharedLibraryInfo")
load(
    "@prelude//python:manifest.bzl",
    "ManifestInfo",
    "create_manifest_for_entries",
    "create_manifest_for_shared_libs",
)
load("@prelude//python:python.bzl", "PythonLibraryInfo")
load("@prelude//python:python_wheel_toolchain.bzl", "PythonWheelToolchainInfo")
load("@prelude//python:toolchain.bzl", "PythonToolchainInfo")
load("@prelude//python:transitions.bzl", "python_transitions")
load("@prelude//transitions:constraint_overrides.bzl", "constraint_overrides")
load("@prelude//utils:expect.bzl", "expect")
load(
    "@prelude//utils:graph_utils.bzl",
    "depth_first_traversal_by",
)
load("@prelude//utils:utils.bzl", "value_or")

def _link_deps(link_infos: dict[Label, LinkableNode], deps: list[Label], link_strategy: LinkStrategy, pic_behavior: PicBehavior) -> list[Label]:
    """
    Return transitive deps required to link dynamically against the given deps.
    This will following through deps of statically linked inputs and exported
    deps of everything else (see https://fburl.com/diffusion/rartsbkw from v1).
    """

    def find_deps(node: Label):
        return get_deps_for_link(link_infos[node], link_strategy, pic_behavior)

    return depth_first_traversal_by(link_infos, deps, find_deps)

def _python_version_from_tag(tag):
    """Extract Python version from a wheel tag: "py3.12" -> "3.12", "cp312" -> "3.12"."""
    version = tag
    for prefix in ("cp", "py"):
        if tag.startswith(prefix):
            version = tag[len(prefix) :]
            break
    if "." in version:
        return version
    if len(version) >= 2:
        return version[0] + "." + version[1:]
    return version

def _cpython_tag(python_version):
    """Convert Python version to CPython tag: "3.12" -> "cp312"."""
    return "cp" + python_version.replace(".", "")

def _whl_cmd(
    ctx: AnalysisContext,
    output: Artifact,
    platform: str,
    abi: str,
    python: str,
    manifests: list[ManifestInfo] = [],
    srcs: dict[str, Artifact] = {},
    computed_metadata: dict[str, str] = {},
    readme: Artifact | None = None,
) -> cmd_args:
    cmd = []

    cmd.append(ctx.attrs._wheel[RunInfo])

    cmd.append("--output")
    cmd.append(output.as_output())
    cmd.append("--name={}".format(ctx.attrs.dist or ctx.attrs.name))
    cmd.append("--version={}".format(ctx.attrs.version))
    cmd.append("--python-tag={}".format(python))
    cmd.append("--abi-tag={}".format(abi))
    cmd.append("--platform-tag={}".format(platform))

    if ctx.attrs.entry_points:
        cmd.append("--entry-points={}".format(json.encode(ctx.attrs.entry_points)))

    for key, val in ctx.attrs.extra_metadata.items():
        cmd.extend(["--metadata", key, val])

    for key, val in computed_metadata.items():
        cmd.extend(["--metadata", key, val])

    for requires in ctx.attrs.requires:
        cmd.extend(["--metadata", "Requires-Dist", requires])

    if readme != None:
        cmd.extend(["--readme", readme])

    for name, script in ctx.attrs.scripts.items():
        cmd.extend(["--data", paths.join("scripts", name), script])

    for dst, src in srcs.items():
        cmd.extend(["--src-path", dst, src])

    hidden = []
    for manifest in manifests:
        cmd.append("--manifest")
        cmd.append(manifest.manifest)
        for a, _ in manifest.artifacts:
            hidden.append(a)

    return cmd_args(cmd, hidden = hidden)

def _rpath(rpath, origin):
    """
    Relative the given `rpath` to `dst`, via `$ORIGIN`.
    If `rpath` is absolute, return it as-is.
    """
    if paths.is_absolute(rpath):
        return rpath

    expect(not paths.is_absolute(origin))

    base = "$ORIGIN"
    if origin:
        base = paths.join(base, *[".." for _ in origin.split("/")])
    return paths.join(base, rpath)

def _impl(ctx: AnalysisContext) -> list[Provider]:
    providers = []
    sub_targets = {}

    libraries = {}
    seen_raw_targets = {}
    for lib in ctx.attrs.libraries:
        libraries[lib.label] = lib
        seen_raw_targets[str(lib.label.raw_target())] = True
    if ctx.attrs.libraries_query != None:
        for lib in ctx.attrs.libraries_query:
            if PythonLibraryInfo in lib:
                raw = str(lib.label.raw_target())
                if raw in seen_raw_targets:
                    # Same target in a different configuration (e.g. due to
                    # a configuration transition in the dep graph).
                    # Keep the first instance.
                    continue
                seen_raw_targets[raw] = True
                libraries[lib.label] = lib

    python_toolchain = ctx.attrs._python_toolchain[PythonToolchainInfo]
    toolchain_info = get_cxx_toolchain_info(ctx)
    dist = ctx.attrs.dist or ctx.attrs.name
    lib_dir = value_or(ctx.attrs.lib_dir, paths.join(dist, "lib"))

    # RPATHs to embed in the binary.
    rpaths = [lib_dir] + ctx.attrs.rpaths + python_toolchain.wheel_rpaths

    def maybe_patchelf(dst, src):
        # If there's no rpaths, there's nothing to patch.
        if not rpaths:
            return src

        # Heuristic: if we see an extension, assume it's not an ELF file (this might miss
        # DSOs embedded as resources).
        _, ext = paths.split_extension(dst)
        if ext:
            return src

        # Run the patchelf -- this will just copy if it turns out the given
        # path isn't and ELF file.
        out = ctx.actions.declare_output(paths.join("__patched__", dst), has_content_based_path = False)
        cmd = cmd_args(
            ctx.attrs._patchelf[RunInfo],
            "--output",
            out.as_output(),
            cmd_args([_rpath(p, origin = paths.dirname(dst)) for p in rpaths], format = "--rpath={}"),
            src,
        )
        ctx.actions.run(cmd, category = "patchelf", identifier = dst)
        return out

    srcs = []
    native_srcs = []
    extensions = {}
    shared_libs = []
    native_deps = {}
    for dep in libraries.values():
        manifests = dep[PythonLibraryInfo].manifests.value
        if manifests.srcs != None:
            srcs.append(manifests.srcs)
        if manifests.default_resources != None:
            # default_resources is a tuple of (manifest, hidden_runtime_files).
            # The manifest ([0]) contains the actual resource files that should
            # be packaged. Hidden runtime files ([1]) are auxiliary artifacts
            # that resources depend on at execution time (e.g., symlink trees
            # for shared libs); they are not directly packageable in a wheel.
            # Previously this asserted [1] was empty, which broke MTIA wheel
            # builds whose deps include native libraries with such auxiliary
            # artifacts. Skipping [1] is safe because the wheel build path
            # bundles shared libs via a separate mechanism.
            srcs.append(manifests.default_resources[0])
        if manifests.extensions != None:
            ((extension, _),) = manifests.extensions.items()
            if extension in extensions:
                existing = extensions[extension]
                if existing.label.raw_target() == dep.label.raw_target():
                    # Same target in a different configuration (e.g. due to
                    # a configuration transition in the dep graph).
                    # Keep the first instance.
                    continue
                fail("Duplicate extension entry for {}. Did your library_query forget to filter by `target_deps()`?".format(extension))
            extensions[extension] = dep

        # These are non-extension native deps that may be dlopen'ed at
        # runtime and should be included in the search space for omnibus roots.
        for native_dep in dep[PythonLibraryInfo].native_deps.value.native_deps.values():
            if SharedLibraryInfo in native_dep:
                native_deps[native_dep.label] = native_dep

    # We support two modes of linking:
    # - omnibus: All native deps of all extensions are linked into a shared DSO
    # - static-everything: Each extension statically links all its deps (note
    #       this can mean each extension gets its own copy of common deps).
    if ctx.attrs.omnibus:
        deps = (extensions | native_deps).values()
        linkable_graph = create_linkable_graph(
            ctx = ctx,
            deps = deps,
        )
        omnibus_graph = get_omnibus_graph(
            graph = linkable_graph,
            roots = get_roots(deps),
            excluded = {},
        )

        extension_labels = {dep.label: None for dep in extensions.values()}

        # Link omnibus libraries.
        omnibus_libs = create_omnibus_libraries(
            ctx = ctx,
            graph = omnibus_graph,
            omnibus_lib_name = "omnibus-{}".format(dist),
            extra_ldflags = python_toolchain.wheel_linker_flags + ctx.attrs.linker_flags,
            extra_root_ldflags = {
                dep.label: (
                    python_toolchain.extension_linker_flags
                    + python_toolchain.wheel_extension_linker_flags
                    + ["-Wl,-rpath,{}".format(_rpath(rpath, origin = paths.dirname(extension))) for rpath in rpaths]
                )
                for extension, dep in extensions.items()
            }
            | {
                # For non-extension roots, set rpaths relative the lib dir.
                root: ["-Wl,-rpath,{}".format(_rpath(rpath, origin = lib_dir)) for rpath in rpaths]
                for root in omnibus_graph.roots.keys()
                if root not in extension_labels
            },
            anonymous = ctx.attrs.anonymous_link,
        )

        # Extract re-linked extensions.
        extensions = {dest: omnibus_libs.roots[dep.label].shared_library for dest, dep in extensions.items()}
        shared_libs = omnibus_libs.libraries
    else:
        for extension, dep in extensions.items():
            root = dep[LinkableRootInfo]

            # Add link inputs for the linkable root and any deps.
            inputs = []
            inputs.append(
                get_link_info(
                    infos = root.link_infos,
                    prefer_stripped = ctx.attrs.prefer_stripped_objects,
                )
            )
            link_infos = get_linkable_graph_node_map_func(dep[LinkableGraph])()
            for ext_dep in _link_deps(
                link_infos,
                root.deps,
                LinkStrategy("static_pic"),
                toolchain_info.pic_behavior,
            ):
                node = link_infos[ext_dep]
                output_style = get_lib_output_style(
                    LinkStrategy("static_pic"),
                    node.preferred_linkage,
                    toolchain_info.pic_behavior,
                )
                inputs.append(
                    get_link_info_for_node(
                        node,
                        output_style,
                        prefer_stripped = ctx.attrs.prefer_stripped_objects,
                    )
                )

                # Record shared libs we need to package.
                if output_style == LibOutputStyle("shared_lib"):
                    shared_libs.extend(node.shared_libs.libraries)

            # link the rule
            link_result = cxx_link_shared_library(
                ctx = ctx,
                output = extension,
                opts = link_options(
                    links = [
                        LinkArgs(flags = python_toolchain.extension_linker_flags),
                        LinkArgs(flags = python_toolchain.wheel_linker_flags),
                        LinkArgs(flags = ["-Wl,-rpath,{}".format(_rpath(rpath, origin = paths.dirname(extension))) for rpath in rpaths]),
                        LinkArgs(flags = ctx.attrs.linker_flags),
                        LinkArgs(infos = inputs),
                    ],
                    category_suffix = "native_extension",
                    identifier = extension,
                    link_execution_preference = LinkExecutionPreference("any"),
                ),
            )
            extensions[extension] = link_result.linked_object

    # Add sub-target for extensions.
    sub_targets["extensions"] = [
        DefaultInfo(
            sub_targets = {name: [DefaultInfo(default_output = ext.output)] for name, ext in extensions.items()},
        ),
    ]
    sub_targets["native-libs"] = [
        DefaultInfo(
            sub_targets = {shlib.soname.ensure_str(): [DefaultInfo(default_output = shlib.lib.output)] for shlib in shared_libs if shlib.soname.is_str},
        ),
    ]

    # Add shlibs manifest.
    if shared_libs:
        # NOTE(agallaher): We copy shared libs/roots, rather than symlink, as
        # some DSO loading frameworks `realpath` the DSO before opening them,
        # causing the `$ORIGIN` in them to no longer be valid, e.g.
        # https://github.com/pytorch/pytorch/blob/main/torch/_utils_internal.py#L62
        native_srcs.append(
            create_manifest_for_shared_libs(
                actions = ctx.actions,
                name = "shared_libs.txt",
                shared_libs = shared_libs,
                lib_dir = lib_dir,
            ),
        )

    if extensions:
        srcs.append(
            create_manifest_for_entries(
                ctx,
                name = "extensions.txt",
                entries = [(name, extension.output, "<unknown>") for name, extension in extensions.items()],
            ),
        )

    # Add in resources for the current rule.
    if ctx.attrs.resources:
        srcs.append(
            create_manifest_for_entries(
                ctx,
                name = "resources.txt",
                entries = [(dest, maybe_patchelf(dest, resource), str(ctx.label.raw_target())) for dest, resource in ctx.attrs.resources.items()],
            ),
        )

    # Resolve platform: use attr if set, otherwise fall back to toolchain default
    wheel_toolchain = ctx.attrs._python_wheel_toolchain[PythonWheelToolchainInfo]
    python = value_or(ctx.attrs.python, wheel_toolchain.python)
    platform = ctx.attrs.platform or wheel_toolchain.platform
    if not platform:
        fail("platform must be set either on python_wheel target or in python_wheel_toolchain")

    # Resolve ABI tag: explicit attr > toolchain default
    abi = value_or(ctx.attrs.abi, wheel_toolchain.abi)

    # Auto-detect CPython tags for wheels with native extensions (PEP 427).
    # When native extensions are present and no explicit ABI override is set,
    # switch from py*-none to cpXXX-cpXXX tags.
    python_version = _python_version_from_tag(python)

    # Normalize python tag to PEP 425 format: "py3.12" -> "py312"
    if "." in python:
        python = python.replace(".", "")

    if extensions and abi == "none":
        cp_tag = _cpython_tag(python_version)
        python = cp_tag
        abi = cp_tag

    # Computed metadata for WHEEL/METADATA files
    computed_metadata = {"Requires-Python": "==" + python_version + ".*"}

    def normalize_name(name):
        # Normalize name part of the *.whl file per:
        #  * https://packaging.python.org/en/latest/specifications/recording-installed-packages/#the-dist-info-directory
        #  * https://packaging.python.org/en/latest/specifications/name-normalization/
        #
        # Equivalent to fbcode/buck2/prelude/python/tools/wheel.py#normalize_name()
        # but need to do it here since we need to set output path

        # PEP503 name normalization
        #   1. Convert to lowercase
        pep503_normalized = name.lower()

        #   2. Replace all dots and underscores with hyphens
        pep503_normalized = pep503_normalized.replace(".", "-")
        pep503_normalized = pep503_normalized.replace("_", "-")

        #   3. Collapse multiple consecutive hyphens into single hyphen
        #      Since we don't have regex, we'll do multiple passes
        for _ in range(pep503_normalized.count("--")):
            pep503_normalized = pep503_normalized.replace("--", "-")

        # Finally replace hyphens with underscores
        return pep503_normalized.replace("-", "_")

    name_parts = [
        # only normalize `dist` in the *.whl filename (NOT the dist name in dist-info/METADATA)
        normalize_name(dist),
        ctx.attrs.version,
        python,
        abi,
        platform,
    ]

    # Action to create wheel.
    wheel = ctx.actions.declare_output("{}.whl".format("-".join(name_parts)), has_content_based_path = False)
    whl_cmd = _whl_cmd(
        ctx = ctx,
        output = wheel,
        platform = platform,
        abi = abi,
        python = python,
        manifests = srcs + native_srcs,
        computed_metadata = computed_metadata,
        readme = ctx.attrs.readme,
    )
    ctx.actions.run(whl_cmd, category = "wheel")

    # Create symlink tree for inplace module layout.
    manifest_args = []
    manifest_srcs = []
    for manifest in srcs:
        manifest_args.append(cmd_args(manifest.manifest, format = "--link-manifest={}"))
        for a, _ in manifest.artifacts:
            manifest_srcs.append(a)
    for manifest in native_srcs:
        manifest_args.append(
            cmd_args(
                manifest.manifest,
                format = "--copy-manifest={}",
                hidden = [a for (a, _) in manifest.artifacts],
            ),
        )
    link_tree = ctx.actions.declare_output("__editable__/tree.d", dir = True, has_content_based_path = False)
    link_tree_cmd = cmd_args(
        ctx.attrs._create_link_tree[RunInfo],
        cmd_args(link_tree.as_output(), format = "--output={}"),
        manifest_args,
    )
    ctx.actions.run(link_tree_cmd, category = "link_tree")

    # Create <dist>.pth to put in the wheel and points to the symlink tree.
    pth = ctx.actions.declare_output("__editable__/{}.pth".format(dist), has_content_based_path = False)
    pth_cmd = cmd_args(
        "sh",
        "-c",
        # We need to embed an absolute paths to the link-tree into the `.pth` file.
        # so we'll also make sure this rule is local-only.
        'echo "$PWD/$2" > $1',
        "",
        pth.as_output(),
        # We don't care about the link tree contents, so prevent it from affecting
        # the cache key.
        cmd_args(link_tree, ignore_artifacts = True),
    )
    ctx.actions.run(pth_cmd, category = "pth", local_only = True)

    # Action to create editable wheel.
    ewheel = ctx.actions.declare_output("__editable__/{}.whl".format("-".join(name_parts)), has_content_based_path = False)
    ewhl_cmd = _whl_cmd(
        ctx = ctx,
        output = ewheel,
        platform = platform,
        abi = abi,
        python = python,
        srcs = {"{}.pth".format(dist): pth},
        computed_metadata = computed_metadata,
        readme = ctx.attrs.readme,
    )
    ctx.actions.run(ewhl_cmd, category = "editable_wheel")
    sub_targets["editable"] = [
        DefaultInfo(
            default_output = ewheel,
            other_outputs = (manifest_srcs + [link_tree]),
        ),
    ]

    providers.append(DefaultInfo(default_output = wheel, sub_targets = sub_targets))

    return providers

python_wheel = rule(
    impl = _impl,
    cfg = constraint_overrides.python_transition,
    attrs = dict(
        dist = attrs.option(attrs.string(), default = None),
        version = attrs.string(default = "1.0.0"),
        python = attrs.option(attrs.string(), default = None),
        abi = attrs.option(attrs.string(), default = None),
        entry_points = attrs.dict(
            key = attrs.string(),
            value = attrs.dict(
                key = attrs.string(),
                value = attrs.string(),
            ),
            default = {},
        ),
        requires = attrs.list(attrs.string(), default = []),
        extra_metadata = attrs.dict(
            key = attrs.string(),
            value = attrs.string(),
            default = {},
        ),
        readme = attrs.option(attrs.source(), default = None),
        platform = attrs.option(
            attrs.string(),
            default = None,
            doc = "Platform tag for the wheel. If not set, uses the toolchain default.",
        ),
        omnibus = attrs.bool(default = False),
        libraries = attrs.list(attrs.dep(providers = [PythonLibraryInfo]), default = []),
        scripts = attrs.dict(key = attrs.string(), value = attrs.source(), default = {}),
        libraries_query = attrs.option(attrs.query(), default = None),
        prefer_stripped_objects = attrs.default_only(attrs.bool(default = False)),
        resources = attrs.dict(key = attrs.string(), value = attrs.source(), default = {}),
        rpaths = attrs.list(attrs.string(), default = []),
        lib_dir = attrs.option(attrs.string(), default = None),
        labels = attrs.list(attrs.string(), default = []),
        deffile = attrs.option(attrs.source(), default = None),
        linker_flags = attrs.list(attrs.arg(anon_target_compatible = True), default = []),
        anonymous_link = attrs.bool(default = True),
        link_execution_preference = link_execution_preference_attr(),
        _wheel = attrs.default_only(attrs.exec_dep(default = "prelude//python/tools:wheel")),
        _patchelf = attrs.default_only(attrs.exec_dep(default = "prelude//python/tools:patchelf")),
        _python_wheel_toolchain = toolchains_common.python_wheel(),
        _create_link_tree = attrs.default_only(attrs.exec_dep(default = "prelude//python/tools:create_link_tree")),
        _cxx_toolchain = toolchains_common.cxx(),
        _python_toolchain = toolchains_common.python(),
    )
    | python_transitions.attrs()
    | constraint_overrides.attributes,
)
