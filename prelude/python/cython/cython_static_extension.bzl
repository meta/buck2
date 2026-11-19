# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//cxx:attrs.bzl", "cxx_library_base_attrs", "cxx_library_extra_attrs")
load(
    "@prelude//cxx:cxx_context.bzl",
    "get_cxx_toolchain_info",
)
load(
    "@prelude//cxx:cxx_library.bzl",
    "cxx_library_parameterized",
)
load(
    "@prelude//cxx:cxx_sources.bzl",
    "CxxSrcWithFlags",
)
load(
    "@prelude//cxx:cxx_types.bzl",
    "CxxRuleConstructorParams",
    "CxxRuleProviderParams",
    "CxxRuleSubTargetParams",
)
load("@prelude//cxx:cxx_utility.bzl", "cxx_attrs_get_allow_cache_upload")
load(
    "@prelude//cxx:headers.bzl",
    "cxx_get_regular_cxx_headers_layout",
)
load(
    "@prelude//cxx:omnibus.bzl",
    "get_roots",
)
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load(
    "@prelude//linking:link_info.bzl",
    "LibOutputStyle",
    "LinkableFlavor",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "create_linkable_graph",
    "create_linkable_graph_node",
)
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")
load("@prelude//python:attrs.bzl", "python_common")
load("@prelude//python:manifest.bzl", "create_manifest_for_source_map")
load(
    "@prelude//python:python.bzl",
    "PythonLibraryInfo",
)
load(
    "@prelude//python:python_library.bzl",
    "create_python_library_info",
    "dest_prefix",
    "gather_dep_libraries",
    "qualify_srcs",
)
load(
    "@prelude//python:source_db.bzl",
    "create_python_source_db_info",
    "create_source_db_no_deps",
)
load("@prelude//python:toolchain.bzl", "PythonToolchainInfo")
load(
    "@prelude//python:versions.bzl",
    "gather_versioned_dependencies",
)
load(
    "@prelude//python/linking:native_python_util.bzl",
    "merge_cxx_extension_info",
    "merge_native_deps",
)
load("@prelude//unix:providers.bzl", "UnixEnv", "create_unix_env_info")
load(
    ":cython_compile.bzl",
    "cython_compile",
)
load(
    ":cython_providers.bzl",
    "CythonIncludesInfo",
    "CythonLibraryInfo",
)
load(
    ":cython_toolchain.bzl",
    "CythonToolchainInfo",
)

def _gather_cython_includes_static(ctx: AnalysisContext, cython_headers, cython_includes, cython_deps: list[Dependency]) -> (Artifact, dict[str, Artifact]):
    """
    Collect Cython include files from the current rule and transitive deps.
    """
    base_module = dest_prefix(ctx.label, ctx.attrs.base_module)

    raw_headers = {}

    # Handle cython_headers - can be list or dict
    if isinstance(cython_headers, dict):
        for name, artifact in cython_headers.items():
            raw_headers[base_module + name] = artifact
    else:
        for hdr in cython_headers:
            raw_headers[base_module + hdr.basename] = hdr

    # Handle cython_includes - can be list or dict
    if isinstance(cython_includes, dict):
        for name, artifact in cython_includes.items():
            raw_headers[base_module + name] = artifact
    else:
        for inc in cython_includes:
            raw_headers[base_module + inc.basename] = inc

    # Collect transitive includes from cython deps
    for dep in cython_deps:
        if CythonIncludesInfo in dep:
            dep_headers = dep[CythonIncludesInfo].raw_headers
            for path, artifact in dep_headers.items():
                if path not in raw_headers:
                    raw_headers[path] = artifact

    # Build __init__.py markers for package directories
    all_dirs = {}
    init_py = ctx.actions.write("__cython_init__/__init__.py", "", has_content_based_path = False)
    for path in raw_headers.keys():
        parts = paths.dirname(path).split("/")
        for i in range(len(parts)):
            if parts[i] and parts[i] != ".":
                dir_path = "/".join(parts[: i + 1])
                if dir_path not in all_dirs:
                    all_dirs[dir_path] = True

    layout = dict(raw_headers)
    for dir_path in all_dirs.keys():
        init_path = dir_path + "/__init__.py"
        if init_path not in layout:
            layout[init_path] = init_py

    include_tree = ctx.actions.symlinked_dir(
        "__cython_includes__",
        layout,
        has_content_based_path = False,
    )

    return include_tree, raw_headers

def cython_static_extension_impl(ctx: AnalysisContext) -> list[Provider]:
    """
    Implementation of the cython_static_extension rule.

    Similar to cython_library but simplified for a single .pyx source,
    producing a statically-linkable extension.
    """
    providers = []
    sub_targets = {}

    # Get the cython toolchain, with optional per-rule compiler override.
    # Python version-based compiler selection is handled via select() on the
    # toolchain's compiler attribute, so no imperative version matching needed.
    cython_toolchain = ctx.attrs._cython_toolchain[CythonToolchainInfo]
    if ctx.attrs.cython_compiler:
        cython_toolchain = CythonToolchainInfo(
            compiler = ctx.attrs.cython_compiler[RunInfo],
            default_flags = cython_toolchain.default_flags,
        )

    generate_cpp = ctx.attrs.cython_generate_cpp
    ext = ".cpp" if generate_cpp else ".c"

    # Build compiler flags
    cython_flags = []

    # Language version
    if ctx.attrs.cython_version in ("3", "3str"):
        cython_flags.append("-3")
    else:
        cython_flags.append("-2")

    # Fast fail
    if ctx.attrs.cython_fast_fail:
        cython_flags.append("--fast-fail")

    # C++ mode
    if generate_cpp:
        cython_flags.append("--cplus")

    # Binding
    cython_flags.extend(["-X", "binding={}".format(ctx.attrs.cython_binding)])

    # Code comments
    cython_flags.extend(["-X", "emit_code_comments={}".format(ctx.attrs.cython_code_comments)])

    # Language level workaround
    cython_flags.extend(["-X", "language_level=3"])

    # Legacy noexcept
    cython_flags.extend(["-X", "legacy_implicit_noexcept={}".format(ctx.attrs.legacy_noexcept)])

    # User-provided flags last
    cython_flags.extend(ctx.attrs.cython_flags)

    # Gather includes
    include_tree, raw_headers = _gather_cython_includes_static(
        ctx = ctx,
        cython_headers = ctx.attrs.cython_headers,
        cython_includes = ctx.attrs.cython_includes,
        cython_deps = ctx.attrs.cython_deps,
    )

    # Compile the single .pyx source
    src = ctx.attrs.cython_pyx
    base_module = dest_prefix(ctx.label, ctx.attrs.base_module)
    package_path = base_module.rstrip("/")
    module_name = paths.split_extension(paths.basename(src.short_path))[0]

    compile_output = cython_compile(
        actions = ctx.actions,
        cython_toolchain = cython_toolchain,
        src = src,
        package_path = package_path,
        include_tree = include_tree,
        flags = cython_flags,
        generate_cpp = generate_cpp,
        identifier = module_name,
        exec_os_type = ctx.attrs._exec_os_type,
    )

    # Sub-targets for generated sources
    gen_src_sub_targets = {
        module_name + ext: [DefaultInfo(default_output = compile_output.cpp_src)],
    }
    gen_header_sub_targets = {
        module_name + "_api.h": [DefaultInfo(default_output = compile_output.api_header)],
        module_name + ".h": [DefaultInfo(default_output = compile_output.public_header)],
    }
    sub_targets["generated-sources"] = [
        DefaultInfo(
            default_outputs = [compile_output.cpp_src],
            sub_targets = gen_src_sub_targets,
        )
    ]
    sub_targets["generated-headers"] = [
        DefaultInfo(
            default_outputs = [compile_output.api_header, compile_output.public_header],
            sub_targets = gen_header_sub_targets,
        )
    ]

    # Build the C++ extension with static linkage preference
    if ctx.attrs._target_os_type[OsLookup].os == Os("windows"):
        library_extension = ".pyd"
    else:
        library_extension = ".so"

    name = module_name + library_extension
    cxx_toolchain = get_cxx_toolchain_info(ctx)
    python_toolchain = ctx.attrs._python_toolchain[PythonToolchainInfo]

    cxx_sub_targets = CxxRuleSubTargetParams(
        argsfiles = True,
        compilation_database = True,
        headers = False,
        link_group_map = False,
        link_style_outputs = False,
        xcode_data = False,
    )

    cxx_providers_params = CxxRuleProviderParams(
        compilation_database = True,
        default = False,
        java_packaging_info = False,
        java_global_code_info = False,
        linkable_graph = False,
        link_style_outputs = False,
        merged_native_link_info = False,
        omnibus_root = True,
        preprocessors = False,
        resources = True,
        shared_libraries = False,
        template_placeholders = False,
        preprocessor_for_tests = False,
    )

    # Combine generated source with any additional C++ sources
    all_srcs = [CxxSrcWithFlags(file = compile_output.cpp_src, flags = [], is_header = False)]
    for additional_src in ctx.attrs.srcs:
        all_srcs.append(CxxSrcWithFlags(file = additional_src, flags = [], is_header = False))

    impl_params = CxxRuleConstructorParams(
        build_empty_so = True,
        rule_type = "cython_static_extension",
        headers_layout = cxx_get_regular_cxx_headers_layout(ctx),
        srcs = all_srcs,
        soname = name,
        use_soname = False,
        generate_providers = cxx_providers_params,
        generate_sub_targets = cxx_sub_targets,
        compiler_flags = ctx.attrs.compiler_flags,
        preprocessor_flags = ctx.attrs.preprocessor_flags,
        extra_link_flags = python_toolchain.extension_linker_flags,
        error_handler = cxx_toolchain.cxx_error_handler,
        allow_cache_upload = cxx_attrs_get_allow_cache_upload(ctx.attrs, cxx_toolchain.cxx_compiler_info.allow_cache_upload),
        _cxx_toolchain = ctx.attrs._cxx_toolchain,
    )

    cxx_library_info = cxx_library_parameterized(ctx, impl_params)
    libraries = cxx_library_info.all_outputs

    solib_default = libraries.solibs.get(LinkableFlavor("default"), None)
    if not solib_default:
        fail("Expected cython_static_extension to produce a solib: {}".format(ctx.label))
    extension = solib_default.linked_object

    providers.extend(cxx_library_info.providers)

    # Build qualified extensions map
    ext_name = module_name + library_extension
    qualified_extensions = qualify_srcs(ctx.label, ctx.attrs.base_module, {ext_name: extension})

    # Type stubs
    src_types = None
    src_type_manifest = None
    type_stubs = {}
    for type_stub in ctx.attrs.python_types:
        type_stubs[type_stub.basename] = type_stub
    if type_stubs:
        qualified_types = qualify_srcs(ctx.label, ctx.attrs.base_module, type_stubs)
        src_type_manifest = create_manifest_for_source_map(
            ctx,
            "type_stubs",
            qualified_types,
        )
        src_types = qualified_types

    # Build PythonLibraryInfo
    raw_deps = ctx.attrs.deps + ctx.attrs.python_deps + ctx.attrs.cython_deps
    deps, shared_deps = gather_dep_libraries(raw_deps, resolve_versioned_deps = False)
    providers.append(gather_versioned_dependencies(raw_deps))

    native_deps = merge_native_deps(ctx, raw_deps)

    library_info = create_python_library_info(
        ctx.actions,
        ctx.label,
        extensions = qualified_extensions,
        deps = deps,
        extension_shared_libraries = shared_deps,
        src_types = src_type_manifest,
        native_deps = native_deps,
        is_native_dep = True,
    )
    providers.append(library_info)

    # CxxExtensionInfo
    providers.append(
        merge_cxx_extension_info(
            actions = ctx.actions,
            deps = raw_deps,
        )
    )

    # Linkable graph
    linkable_graph = create_linkable_graph(
        ctx,
        node = create_linkable_graph_node(
            ctx,
            roots = get_roots([dep for dep in raw_deps if PythonLibraryInfo in dep]),
        ),
        deps = raw_deps,
    )
    providers.append(linkable_graph)

    # UnixEnv
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

    # CythonIncludesInfo
    cython_includes_info = CythonIncludesInfo(
        include_tree = include_tree,
        raw_headers = raw_headers,
    )
    providers.append(cython_includes_info)

    # CythonLibraryInfo with declaration headers
    declaration_hdrs = {
        base_module + module_name + "_api.h": compile_output.api_header,
        base_module + module_name + ".h": compile_output.public_header,
    }
    providers.append(
        CythonLibraryInfo(
            declaration_headers = declaration_hdrs,
            include_info = cython_includes_info,
        )
    )

    # Source DB
    if src_types:
        sub_targets["source-db-no-deps"] = [
            create_source_db_no_deps(ctx, src_types),
            create_python_source_db_info(library_info.manifests),
        ]

    # Default output
    shared_output = libraries.outputs.get(LibOutputStyle("shared_lib"), {})
    default_flavor = shared_output.get(LinkableFlavor("default"), None)
    default_output = default_flavor.default if default_flavor else compile_output.cpp_src

    providers.append(
        DefaultInfo(
            default_output = default_output,
            sub_targets = sub_targets,
        )
    )

    return providers

cython_static_extension = prelude_rule(
    name = "cython_static_extension",
    impl = cython_static_extension_impl,
    docs = """
        A rule that compiles a single Cython (.pyx) source into a statically-linkable
        Python C extension.

        Similar to cython_library but simplified for single-source use and
        produces extensions suitable for static linking into binaries.
    """,
    attrs = (
        # Inherit all cxx_library attrs as default_only so cxx_library_parameterized
        # can access required attributes like supported_platforms_regex, preferred_linkage, etc.
        # @unsorted-dict-items
        {k: attrs.default_only(v) for k, v in cxx_library_base_attrs().items()}
        | python_common.base_module_arg()
        | {
            "compiler_flags": attrs.list(
                attrs.arg(),
                default = [],
                doc = """
                Additional C++ compiler flags.
            """,
            ),
            "cython_binding": attrs.bool(
                default = False,
                doc = """
                Whether this is a Cython binding module.
            """,
            ),
            "cython_code_comments": attrs.bool(
                default = False,
                doc = """
                Whether to include source code comments in the generated output.
            """,
            ),
            "cython_compiler": attrs.option(
                attrs.exec_dep(providers = [RunInfo]),
                default = None,
                doc = """
                Override the default Cython compiler.
            """,
            ),
            "cython_deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                Cython-specific dependencies (other cython_library or
                cython_static_extension targets).
            """,
            ),
            "cython_fast_fail": attrs.bool(
                default = True,
                doc = """
                Whether to enable --fast-fail for Cython compilation.
            """,
            ),
            "cython_flags": attrs.list(
                attrs.string(),
                default = [],
                doc = """
                Additional flags to pass to the Cython compiler.
            """,
            ),
            "cython_generate_cpp": attrs.bool(
                default = True,
                doc = """
                Whether to generate C++ (.cpp) or C (.c) output.
            """,
            ),
            "cython_headers": attrs.one_of(
                attrs.list(attrs.source()),
                attrs.dict(attrs.string(), attrs.source()),
                doc = """
                    Cython header files (.pxd) for this extension.
                    Can be a list of sources or a dict mapping names to sources.
                """,
            ),
            "cython_includes": attrs.one_of(
                attrs.list(attrs.source()),
                attrs.dict(attrs.string(), attrs.source()),
                doc = """
                    Cython include files (.pxd, .pxi) for this extension.
                    Can be a list of sources or a dict mapping names to sources.
                """,
            ),
            "cython_pyx": attrs.source(
                doc = """
                The single Cython .pyx source file.
            """
            ),
            "cython_pyx_on_disk": attrs.option(
                attrs.source(),
                default = None,
                doc = """
                Optional on-disk override for the .pyx source.
            """,
            ),
            "cython_version": attrs.enum(
                ["2", "3", "3str"],
                default = "3",
                doc = """
                The Cython language version to target.
            """,
            ),
            "deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                C++ library dependencies.
            """,
            ),
            "legacy_noexcept": attrs.bool(
                default = True,
                doc = """
                Whether to use legacy implicit noexcept behavior.
            """,
            ),
            # Override inherited cxx_library attrs that need concrete defaults
            "preferred_linkage": attrs.enum(["any", "static", "shared"], default = "any"),
            "preprocessor_flags": attrs.list(
                attrs.arg(),
                default = [],
                doc = """
                Additional C++ preprocessor flags.
            """,
            ),
            "public_deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                Public C++ dependencies that are re-exported.
            """,
            ),
            "public_include_directories": attrs.list(
                attrs.string(),
                default = [],
                doc = """
                Public include directories for C++ consumers.
            """,
            ),
            "python_deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                Python dependencies.
            """,
            ),
            "python_types": attrs.list(
                attrs.source(),
                default = [],
                doc = """
                Type stub files (.pyi) for the generated extension.
            """,
            ),
            "raw_headers": attrs.list(
                attrs.source(),
                default = [],
                doc = """
                Raw C/C++ header files.
            """,
            ),
            # cxx attrs
            "srcs": attrs.list(
                attrs.source(),
                default = [],
                doc = """
                Additional C/C++ source files to compile alongside the
                Cython-generated source.
            """,
            ),
            # Toolchain attrs
            "_cxx_toolchain": toolchains_common.cxx(),
            "_cython_toolchain": toolchains_common.cython(),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_python_toolchain": toolchains_common.python(),
            "_target_os_type": buck.target_os_type_arg(),
        }
        | cxx_library_extra_attrs()
    ),
)
