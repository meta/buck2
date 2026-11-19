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
load(
    "@prelude//third-party:build.bzl",
    "create_third_party_build_root",
    "prefix_from_label",
)
load("@prelude//third-party:providers.bzl", "ThirdPartyBuild", "third_party_build_info")
load("@prelude//unix:providers.bzl", "UnixEnv", "create_unix_env_info")
load(
    ":cython_compile.bzl",
    "cython_compile",
    "generate_cython_flags",
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

def _srcs_to_artifacts(srcs) -> list[Artifact]:
    """Convert named_set (list or dict) to a list of artifacts."""
    if isinstance(srcs, dict):
        return srcs.values()
    return srcs

def _gather_cython_includes(ctx: AnalysisContext, srcs, headers, includes, deps: list[Dependency]) -> (Artifact, dict[str, Artifact]):
    """
    Collect all Cython include files (.pxd, .pxi) from the current rule
    and transitive dependencies, and create a symlinked directory for includes.
    """
    base_module = dest_prefix(ctx.label, ctx.attrs.base_module)

    # Collect raw headers for this rule
    raw_headers = {}
    for src in _srcs_to_artifacts(srcs):
        _, ext = paths.split_extension(src.short_path)
        if ext in (".pxd", ".pxi"):
            raw_headers[base_module + src.basename] = src
    for hdr in _srcs_to_artifacts(headers):
        raw_headers[base_module + hdr.basename] = hdr
    for inc in _srcs_to_artifacts(includes):
        raw_headers[base_module + inc.basename] = inc

    # Collect transitive includes from deps
    for dep in deps:
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

    # Create the symlinked dir with headers + init files
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

def _compile_cython_sources(
    ctx: AnalysisContext, cython_toolchain: CythonToolchainInfo, srcs, include_tree: Artifact, flags: list[str], generate_cpp: bool
) -> dict:
    """
    Compile all .pyx sources using the Cython compiler.
    Returns a dict of {module_name: CythonCompileOutput}.
    """
    base_module = dest_prefix(ctx.label, ctx.attrs.base_module)

    # Remove trailing slash for use as package_path
    package_path = base_module.rstrip("/")

    compiled = {}
    for src in _srcs_to_artifacts(srcs):
        _, ext = paths.split_extension(src.short_path)
        if ext != ".pyx":
            continue

        module_name = paths.split_extension(paths.basename(src.short_path))[0]
        identifier = module_name

        compile_output = cython_compile(
            actions = ctx.actions,
            cython_toolchain = cython_toolchain,
            src = src,
            package_path = package_path,
            include_tree = include_tree,
            flags = flags,
            generate_cpp = generate_cpp,
            identifier = identifier,
            exec_os_type = ctx.attrs._exec_os_type,
        )
        compiled[module_name] = compile_output

    return compiled

def _build_cxx_python_extension(ctx: AnalysisContext, module_name: str, generated_src: Artifact, cpp_compiler_flags: list, cpp_preprocessor_flags: list):
    """
    Build a single C++ Python extension from a generated .cpp/.c file.

    Returns the linked extension output and associated providers.
    """
    if ctx.attrs._target_os_type[OsLookup].os == Os("windows"):
        library_extension = ".pyd"
    else:
        library_extension = ".so"

    name = module_name + library_extension
    cxx_toolchain = get_cxx_toolchain_info(ctx)
    python_toolchain = ctx.attrs._python_toolchain[PythonToolchainInfo]

    sub_targets = CxxRuleSubTargetParams(
        argsfiles = True,
        compilation_database = True,
        headers = False,
        link_group_map = False,
        link_style_outputs = False,
        xcode_data = False,
    )

    cxx_providers = CxxRuleProviderParams(
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

    srcs = [CxxSrcWithFlags(file = generated_src, flags = [], is_header = False)]

    impl_params = CxxRuleConstructorParams(
        build_empty_so = True,
        rule_type = "cython_library",
        headers_layout = cxx_get_regular_cxx_headers_layout(ctx),
        srcs = srcs,
        soname = name,
        use_soname = False,
        generate_providers = cxx_providers,
        generate_sub_targets = sub_targets,
        compiler_flags = cpp_compiler_flags,
        preprocessor_flags = cpp_preprocessor_flags,
        extra_link_flags = python_toolchain.extension_linker_flags,
        error_handler = cxx_toolchain.cxx_error_handler,
        allow_cache_upload = cxx_attrs_get_allow_cache_upload(ctx.attrs, cxx_toolchain.cxx_compiler_info.allow_cache_upload),
        _cxx_toolchain = ctx.attrs._cxx_toolchain,
    )

    cxx_library_info = cxx_library_parameterized(ctx, impl_params)
    libraries = cxx_library_info.all_outputs

    solib_default = libraries.solibs.get(LinkableFlavor("default"), None)
    if not solib_default:
        fail("Expected cython_library extension to produce a solib for module: {}".format(module_name))
    extension = solib_default.linked_object

    return extension, cxx_library_info

def cython_library_impl(ctx: AnalysisContext) -> list[Provider]:
    """
    Implementation of the cython_library rule.

    High-level flow:
    1. Gather transitive Cython includes (.pxd/.pxi) from deps
    2. Compile each .pyx source to .cpp/.c using the Cython compiler
    3. Build each generated file as a C++ Python extension
    4. Assemble Python library info with extensions + type stubs
    5. Return all providers (DefaultInfo, PythonLibraryInfo, CythonIncludesInfo, etc.)
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

    # Determine source extension
    generate_cpp = ctx.attrs.generate_cpp
    ext = ".cpp" if generate_cpp else ".c"

    # Build compiler flags
    flags = generate_cython_flags(
        generate_cpp = generate_cpp,
        cython_binding = ctx.attrs.cython_binding,
        code_comments = ctx.attrs.code_comments,
        legacy_noexcept = ctx.attrs.legacy_noexcept,
        annotate = ctx.attrs.cython_annotate,
        flags = ctx.attrs.flags,
    )

    # Gather all Cython include files from current rule and deps
    include_tree, raw_headers = _gather_cython_includes(
        ctx = ctx,
        srcs = ctx.attrs.srcs,
        headers = ctx.attrs.headers,
        includes = ctx.attrs.includes,
        deps = ctx.attrs.deps,
    )

    # Compile all .pyx sources
    compiled_modules = _compile_cython_sources(
        ctx = ctx,
        cython_toolchain = cython_toolchain,
        srcs = ctx.attrs.srcs,
        include_tree = include_tree,
        flags = flags,
        generate_cpp = generate_cpp,
    )

    # Build generated-sources and generated-headers sub-targets for debugging
    gen_src_sub_targets = {}
    gen_header_sub_targets = {}
    all_gen_srcs = []
    all_declaration_headers = []

    for module_name, compile_output in compiled_modules.items():
        all_gen_srcs.append(compile_output.cpp_src)
        gen_src_sub_targets[module_name + ext] = [
            DefaultInfo(
                default_output = compile_output.cpp_src,
            )
        ]

        gen_header_sub_targets[module_name + "_api.h"] = [
            DefaultInfo(
                default_output = compile_output.api_header,
            )
        ]
        gen_header_sub_targets[module_name + ".h"] = [
            DefaultInfo(
                default_output = compile_output.public_header,
            )
        ]

        # Collect declaration headers for api modules
        if module_name in ctx.attrs.api:
            all_declaration_headers.append(compile_output.api_header)
            all_declaration_headers.append(compile_output.public_header)

    sub_targets["generated-sources"] = [
        DefaultInfo(
            default_outputs = all_gen_srcs,
            sub_targets = gen_src_sub_targets,
        )
    ]
    sub_targets["generated-headers"] = [
        DefaultInfo(
            default_outputs = all_declaration_headers,
            sub_targets = gen_header_sub_targets,
        )
    ]

    # Build C++ Python extensions for each compiled module
    base_module = dest_prefix(ctx.label, ctx.attrs.base_module)
    if ctx.attrs._target_os_type[OsLookup].os == Os("windows"):
        library_extension = ".pyd"
    else:
        library_extension = ".so"

    all_extensions = {}
    all_cxx_library_infos = []

    for module_name, compile_output in compiled_modules.items():
        extension, cxx_library_info = _build_cxx_python_extension(
            ctx = ctx,
            module_name = module_name,
            generated_src = compile_output.cpp_src,
            cpp_compiler_flags = ctx.attrs.cpp_compiler_flags,
            cpp_preprocessor_flags = ctx.attrs.cpp_preprocessor_flags,
        )

        ext_name = module_name + library_extension
        all_extensions[ext_name] = extension
        all_cxx_library_infos.append(cxx_library_info)

    # Qualify extensions with base module prefix
    qualified_extensions = qualify_srcs(ctx.label, ctx.attrs.base_module, all_extensions)

    # Add type stubs if present
    src_types = None
    src_type_manifest = None
    type_stubs = {}
    for type_stub in ctx.attrs.types:
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
    raw_deps = ctx.attrs.deps + ctx.attrs.python_deps
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

    # CxxExtensionInfo for omnibus linking
    providers.append(
        merge_cxx_extension_info(
            actions = ctx.actions,
            deps = raw_deps + ctx.attrs.cpp_deps,
        )
    )

    # Linkable graph
    linkable_graph = create_linkable_graph(
        ctx,
        node = create_linkable_graph_node(
            ctx,
            roots = get_roots([dep for dep in raw_deps if PythonLibraryInfo in dep]),
        ),
        deps = raw_deps + ctx.attrs.cpp_deps,
    )
    providers.append(linkable_graph)

    # UnixEnv provider
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

    # Allow third-party-build rules to depend on cython_library.
    tp_prefix = prefix_from_label(ctx.label)
    providers.append(
        third_party_build_info(
            actions = ctx.actions,
            build = ThirdPartyBuild(
                prefix = tp_prefix,
                root = create_third_party_build_root(
                    ctx = ctx,
                    paths = [(paths.join("lib/python", base_module + ext_name), ext.output) for ext_name, ext in all_extensions.items()],
                ),
                manifest = ctx.actions.write_json(
                    "third_party_build_manifest.json",
                    dict(
                        bin_paths = [],
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
            deps = raw_deps,
        ),
    )

    # CythonIncludesInfo for downstream cython_library deps
    cython_includes_info = CythonIncludesInfo(
        include_tree = include_tree,
        raw_headers = raw_headers,
    )
    providers.append(cython_includes_info)

    # CythonLibraryInfo for C++ interop
    declaration_hdrs = {}
    for module_name, compile_output in compiled_modules.items():
        if module_name in ctx.attrs.api:
            declaration_hdrs[base_module + module_name + "_api.h"] = compile_output.api_header
            declaration_hdrs[base_module + module_name + ".h"] = compile_output.public_header

    providers.append(
        CythonLibraryInfo(
            declaration_headers = declaration_hdrs,
            include_info = cython_includes_info,
        )
    )

    # Source DB sub-targets
    if src_types:
        sub_targets["source-db-no-deps"] = [
            create_source_db_no_deps(ctx, src_types),
            create_python_source_db_info(library_info.manifests),
        ]

    # Default output - if there are extensions, use the first one
    default_output = None
    if all_gen_srcs:
        default_output = all_gen_srcs[0]

    providers.append(
        DefaultInfo(
            default_output = default_output,
            sub_targets = sub_targets,
        )
    )

    return providers

cython_library = prelude_rule(
    name = "cython_library",
    impl = cython_library_impl,
    docs = """
        A rule that compiles Cython (.pyx) sources into Python C extensions.

        This rule handles the full pipeline: Cython transpilation to C/C++,
        compilation to shared libraries, and assembly into a Python library.
    """,
    attrs = (
        # Inherit all cxx_library attrs as default_only so cxx_library_parameterized
        # can access required attributes like supported_platforms_regex, preferred_linkage, etc.
        # This mirrors how cxx_python_extension inherits these attrs.
        # @unsorted-dict-items
        {k: attrs.default_only(v) for k, v in cxx_library_base_attrs().items()}
        | python_common.base_module_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | {
            "allow_embedding": attrs.option(
                attrs.bool(),
                default = None,
                doc = """
                Whether to allow embedding this extension into the main binary.
            """,
            ),
            "api": attrs.list(
                attrs.string(),
                default = [],
                doc = """
                List of module names that should generate public API headers
                (_api.h files) for C++ interop.
            """,
            ),
            "code_comments": attrs.bool(
                default = False,
                doc = """
                Whether to include source code comments in the generated output.
            """,
            ),
            "cpp_compiler_flags": attrs.list(
                attrs.arg(),
                default = [],
                doc = """
                Additional flags to pass to the C++ compiler.
            """,
            ),
            "cpp_deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                C++ library dependencies for linking.
            """,
            ),
            "cpp_external_deps": attrs.list(
                attrs.string(),
                default = [],
                doc = """
                External C++ dependencies specified by name.
            """,
            ),
            "cpp_preprocessor_flags": attrs.list(
                attrs.arg(),
                default = [],
                doc = """
                Additional C++ preprocessor flags.
            """,
            ),
            "cpp_python_extension_srcs": attrs.list(
                attrs.source(),
                default = [],
                doc = """
                Additional C++ source files to compile into the Python extensions.
            """,
            ),
            "cython_annotate": attrs.option(
                attrs.enum(["off", "basic", "fullc"]),
                default = None,
                doc = """
                Level of Cython annotation to generate.
            """,
            ),
            "cython_binding": attrs.bool(
                default = False,
                doc = """
                Whether this is a Cython binding module.
            """,
            ),
            "cython_compiler": attrs.option(
                attrs.exec_dep(providers = [RunInfo]),
                default = None,
                doc = """
                Override the default Cython compiler.
            """,
            ),
            "cython_so_package": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                Override the package path for the generated .so file.
            """,
            ),
            "deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                Dependencies of this rule, including other cython_library
                and python_library targets.
            """,
            ),
            "flags": attrs.list(
                attrs.string(),
                default = [],
                doc = """
                Additional flags to pass to the Cython compiler.
            """,
            ),
            "generate_cpp": attrs.bool(
                default = True,
                doc = """
                Whether to generate C++ (.cpp) or C (.c) output from Cython.
            """,
            ),
            "header_namespace": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                A namespace for the exported headers. If set, headers will be
                available under this namespace for dependent rules.
            """,
            ),
            "headers": attrs.named_set(
                attrs.source(),
                sorted = True,
                default = [],
                doc = """
                Cython header files (.pxd) to expose to dependent Cython rules.
            """,
            ),
            "includes": attrs.named_set(
                attrs.source(),
                sorted = True,
                default = [],
                doc = """
                Additional Cython include files (.pxd, .pxi) for this library.
            """,
            ),
            "legacy_noexcept": attrs.bool(
                default = True,
                doc = """
                Whether to use legacy implicit noexcept behavior.
            """,
            ),
            "need_static_lib": attrs.bool(
                default = False,
                doc = """
                Whether to also produce a static C++ library.
            """,
            ),
            # Override inherited cxx_library attrs that need concrete defaults
            "preferred_linkage": attrs.enum(["any", "static", "shared"], default = "any"),
            "public_declarations": attrs.list(
                attrs.string(),
                default = [],
                doc = """
                List of public declaration module names.
            """,
            ),
            "python_deps": attrs.list(
                attrs.dep(),
                default = [],
                doc = """
                Python-only dependencies.
            """,
            ),
            "python_external_deps": attrs.list(
                attrs.string(),
                default = [],
                doc = """
                External Python dependencies specified by name.
            """,
            ),
            "srcs": attrs.named_set(
                attrs.source(),
                sorted = True,
                default = [],
                doc = """
                The Cython source files (.pyx) and header files (.pxd, .pxi)
                to include in this library.
            """,
            ),
            "suffix_all": attrs.option(
                attrs.bool(),
                default = None,
                doc = """
                Whether to suffix all symbols for static linking.
            """,
            ),
            "types": attrs.list(
                attrs.source(),
                default = [],
                doc = """
                Type stub files (.pyi) for the generated modules.
            """,
            ),
            "typing": attrs.bool(
                default = False,
                doc = """
                Whether to enable type checking support.
            """,
            ),
            # Private/toolchain attrs
            "_cxx_toolchain": toolchains_common.cxx(),
            "_cython_toolchain": toolchains_common.cython(),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_python_toolchain": toolchains_common.python(),
            "_target_os_type": buck.target_os_type_arg(),
        }
        | cxx_library_extra_attrs()
    ),
)
