# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load(
    "@prelude//:resources.bzl",
    "gather_resources",
    "make_resource_info",
)
load(
    "@prelude//cxx:omnibus.bzl",
    "get_excluded",
    "get_roots",
)
load(
    "@prelude//cxx:preprocessor.bzl",
    "CPreprocessor",
    "CPreprocessorArgs",
    "cxx_inherited_preprocessor_infos",
    "cxx_merge_cpreprocessors",
    "format_system_include_arg",
)
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:third_party_common.bzl", "third_party_common")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load(
    "@prelude//linking:linkable_graph.bzl",
    "create_linkable_graph",
    "create_linkable_graph_node",
)
load("@prelude//python:attrs.bzl", "create_manifest_for_source_dir_attr", "python_common")
load(
    "@prelude//third-party:build.bzl",
    "create_third_party_build_root",
    "prefix_from_label",
)
load("@prelude//third-party:providers.bzl", "ThirdPartyBuild", "third_party_build_info")
load("@prelude//unix:providers.bzl", "UnixEnv", "create_unix_env_info")
load(":compile.bzl", "compile_manifests")
load(":lazy_imports.bzl", "get_lazy_imports_analyzer", "run_lazy_imports_library_analyzer")
load(":manifest.bzl", "create_manifest_for_source_dir", "new_manifest_info")
load(":python.bzl", "NativeDepsInfo", "NativeDepsInfoTSet")
load(
    ":python_library.bzl",
    "create_python_library_info",
    "gather_dep_libraries",
)
load(":source_db.bzl", "create_python_source_db_info", "create_source_db_no_deps_from_manifest")
load(":toolchain.bzl", "PythonToolchainInfo")

def prebuilt_python_library_impl(ctx: AnalysisContext) -> list[Provider]:
    providers = []

    binary_src = ctx.attrs.binary_src
    source_dir = ctx.attrs.source_dir
    if (binary_src == None) == (source_dir == None):
        fail("exactly one of `binary_src` or `source_dir` must be set")
    if source_dir != None and ctx.attrs.strip_soabi_tags:
        fail("`strip_soabi_tags` is only supported with `binary_src`")

    # Archives must be extracted before their contents can be manifested. A
    # source directory can be manifested directly, while still using the
    # preparation tool to discover entry points and optional C++ headers.
    entry_points = ctx.actions.declare_output("entry_points.manifest", has_content_based_path = False)
    entry_points_dir = ctx.actions.declare_output("__entry_points__", dir = True, has_content_based_path = False)
    if binary_src != None:
        source = binary_src
        source_root = ctx.actions.declare_output("{}_extracted".format(ctx.label.name), dir = True, has_content_based_path = False)
        cmd = cmd_args(
            ctx.attrs._extract[RunInfo],
            binary_src,
            "--output",
            source_root.as_output(),
            "--entry-points-manifest",
            entry_points.as_output(),
            "--entry-points",
            entry_points_dir.as_output(),
        )
    else:
        source = source_dir
        source_root = source_dir
        cmd = cmd_args(
            ctx.attrs._extract[RunInfo],
            source_dir,
            "--entry-points-manifest",
            entry_points.as_output(),
            "--entry-points",
            entry_points_dir.as_output(),
        )
    if ctx.attrs.strip_soabi_tags:
        cmd.add("--strip-soabi-tags")
    inferred_cxx_header_dirs = None
    if ctx.attrs.infer_cxx_header_dirs:
        inferred_cxx_header_dirs = ctx.actions.declare_output("__cxx_header_dirs__.txt", has_content_based_path = False)
        cmd.add(
            "--cxx-header-dirs",
            inferred_cxx_header_dirs.as_output(),
        )
    ctx.actions.run(cmd, category = "py_prepare_prebuilt_library")
    deps, shared_deps = gather_dep_libraries(ctx.attrs.deps)
    manifest_param = "binary_src" if binary_src != None else "source_dir"
    src_manifest = create_manifest_for_source_dir(ctx, manifest_param, source_root, exclude = "\\.pyc$")
    bytecode = None
    if ctx.attrs._python_toolchain[PythonToolchainInfo].pyc_compilation_enabled:
        bytecode = compile_manifests(ctx, [src_manifest])

    lazy_imports_analyzer = get_lazy_imports_analyzer(ctx)
    lazy_imports_cache_output = None
    if lazy_imports_analyzer != None and getattr(ctx.attrs, "use_lifeguard_incremental", False):
        lazy_imports_cache_output = ctx.actions.declare_output("safer_lazy_imports/library-cache.bin")

    library_info = create_python_library_info(
        ctx.actions,
        ctx.label,
        srcs = src_manifest,
        src_types = src_manifest,
        bytecode = bytecode,
        deps = deps,
        shared_libraries = shared_deps,
        native_deps = ctx.actions.tset(
            NativeDepsInfoTSet,
            value = NativeDepsInfo(native_deps = {}),
            children = [],
        ),
        is_native_dep = False,
        lazy_imports_cache = lazy_imports_cache_output,
    )
    providers.append(library_info)

    entry_points_manifest = new_manifest_info(
        manifest = entry_points,
        artifacts = [(entry_points_dir, "")],
    )

    # Create, augment and provide the linkable graph.
    linkable_graph = create_linkable_graph(
        ctx,
        node = create_linkable_graph_node(
            ctx,
            roots = get_roots(ctx.attrs.deps),
            excluded = get_excluded(deps = ctx.attrs.deps if ctx.attrs.exclude_deps_from_merged_linking else []),
        ),
        deps = ctx.attrs.deps,
    )
    providers.append(linkable_graph)

    source_db_no_deps = create_source_db_no_deps_from_manifest(ctx, src_manifest)
    sub_targets = {"source-db-no-deps": [source_db_no_deps, create_python_source_db_info(library_info.manifests)]}

    if lazy_imports_cache_output != None:
        run_lazy_imports_library_analyzer(
            ctx,
            lazy_imports_analyzer,
            lazy_imports_cache_output,
            source_db_no_deps,
        )
        sub_targets["lazy-import-cache"] = [DefaultInfo(default_output = lazy_imports_cache_output)]
    providers.append(DefaultInfo(default_output = source, sub_targets = sub_targets))

    # C++ resources.
    providers.append(
        make_resource_info(
            gather_resources(
                label = ctx.label,
                deps = ctx.attrs.deps,
            ),
        ),
    )

    # Allow third-party-build rules to depend on Python rules.
    tp_prefix = prefix_from_label(ctx.label)
    providers.append(
        third_party_build_info(
            actions = ctx.actions,
            build = ThirdPartyBuild(
                prefix = tp_prefix,
                root = create_third_party_build_root(
                    ctx = ctx,
                    paths = [
                        ("lib/python", source_root),
                    ],
                    manifests = [
                        ("bin", entry_points_manifest),
                    ],
                ),
                manifest = ctx.actions.write_json(
                    "third_party_build_manifest.json",
                    dict(
                        c_include_paths = [],
                        cxx_include_paths = [],
                        lib_paths = [],
                        libs = [],
                        prefix = tp_prefix,
                        py_lib_paths = ["lib/python"],
                        runtime_lib_paths = [],
                    ),
                    has_content_based_path = False,
                ),
            ),
            deps = ctx.attrs.deps,
        ),
    )

    # Unix env provider.
    providers.append(
        create_unix_env_info(
            actions = ctx.actions,
            env = UnixEnv(
                label = ctx.label,
                python_libs = [library_info],
                binaries = [entry_points_manifest],
            ),
            deps = ctx.attrs.deps,
        ),
    )

    # If this prebuilt wheel contains headers, export them via a C++ provider.
    pp_args = []
    if ctx.attrs.cxx_header_dirs:
        for header_dir in ctx.attrs.cxx_header_dirs:
            pp_args.append(
                format_system_include_arg(
                    cmd_args(source_root.project(header_dir)),
                    "clang",
                ),
            )
    if inferred_cxx_header_dirs != None:
        pp_argsfile = ctx.actions.declare_output("__cxx_header_dirs__.py_cxx_header_argsfile", has_content_based_path = False)

        def write_argsfile(actions, header_dirs, output):
            lines = []
            for header_dir in header_dirs.read_string().splitlines():
                lines.append(
                    format_system_include_arg(
                        cmd_args(source_root.project(header_dir)),
                        "clang",
                    )
                )
            actions.write(output, lines)

        ctx.actions.dynamic_output(
            dynamic = [inferred_cxx_header_dirs],
            inputs = [],
            outputs = [pp_argsfile.as_output()],
            f = lambda ctx, artifacts, outputs: write_argsfile(
                ctx.actions,
                artifacts[inferred_cxx_header_dirs],
                outputs[pp_argsfile],
            ),
        )
        pp_args.append(
            cmd_args(
                pp_argsfile,
                format = "@{}",
                hidden = [source_root],
            ),
        )
    if pp_args:
        providers.append(
            cxx_merge_cpreprocessors(
                actions = ctx.actions,
                own = [
                    CPreprocessor(
                        args = CPreprocessorArgs(
                            args = pp_args,
                        ),
                    ),
                ],
                xs = cxx_inherited_preprocessor_infos(ctx.attrs.deps),
            )
        )

    return providers

prebuilt_python_library = prelude_rule(
    name = "prebuilt_python_library",
    impl = prebuilt_python_library_impl,
    docs = """
        A `prebuilt_python_library()` rule is used to include prebuilt python packages into the output of a
        top-level `python_binary()` or `python_test()` rule.

        These prebuilt libraries can either be [whl files](https://www.python.org/dev/peps/pep-0427/) or eggs

        whls for most packages are available for download from [PyPI](https://pypi.org). The whl used may be
        downloaded with `remote_file()`. However, Buck does not attempt to infer dependency information from pip,
        so that information will have to be imparted by the user.

        To create an egg for a package, run `python setup.py bdist_egg` in the package source distribution.
    """,
    examples = """
        ```
        # A simple prebuilt_python_library with no external dependencies.
        remote_file(
          name = "requests-download",
          url = "https://files.pythonhosted.org/packages/51/bd/23c926cd341ea6b7dd0b2a00aba99ae0f828be89d72b2190f27c11d4b7fb/requests-2.22.0-py2.py3-none-any.whl",
          sha1 = "e1fc28120002395fe1f2da9aacea4e15a449d9ee",
          out = "requests-2.22.0-py2.py3-none-any.whl",
        )

        prebuilt_python_library(
          name = "requests",
          binary_src = ":requests-download",
        )

        # A slightly more complex example
        prebuilt_python_library(
          name = "greenlet",
          binary_src = "greenlet-0.4.7-py2.7-macosx-10.10-x86_64.egg",
        )

        prebuilt_python_library(
          name = "gevent",
          binary_src = "gevent-1.0.2-py2.7-macosx-10.10-x86_64.egg",
          deps = [
            ":greenlet",
          ],
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        buck.labels_arg()
        | {
            "binary_src": attrs.option(
                attrs.source(),
                default = None,
                doc = """
                The path to the `.whl`, `.egg`, or `.tar.gz` archive to extract.

                 Exactly one of `binary_src` and `source_dir` must be set. Note:
                 `.egg` files have a very particular naming convention that must
                 be followed, otherwise they will not be found at runtime.
            """,
            ),
            "source_dir": attrs.option(
                attrs.source(allow_directory = True),
                default = None,
                doc = """
                A directory containing an already-materialized Python package.

                 The directory is manifested in place without being copied or
                 archived. Exactly one of `binary_src` and `source_dir` must be set.
            """,
            ),
        }
        | python_common.deps_arg()
        | python_common.exclude_deps_from_merged_linking_arg()
        | third_party_common.create_third_party_build_root_attrs()
        | {
            "cxx_header_dirs": attrs.option(attrs.list(attrs.string()), default = None),
            "infer_cxx_header_dirs": attrs.bool(default = False),
            "lazy_imports_analyzer": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
            "strip_soabi_tags": attrs.bool(
                default = False,
                doc = """
                    Strip the SOABI tags from extensions in the prebuilt library.

                    Note that this should be considered unsafe, as it removes builtin
                    protections that fail fast when a potententially incompatible
                    native extension is imported.
                """,
            ),
            "use_lifeguard_incremental": attrs.bool(default = False),
            "_create_manifest_for_source_dir": create_manifest_for_source_dir_attr(),
            "_extract": attrs.default_only(attrs.exec_dep(default = "prelude//python/tools:extract")),
            "_python_internal_tools": python_common.internal_tools_arg(),
            "_python_toolchain": toolchains_common.python(),
        }
        | buck.licenses_arg()
        | buck.contacts_arg()
    ),
)
