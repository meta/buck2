# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//cxx:attrs.bzl", "cxx_common", "cxx_library_base_attrs")
load(
    "@prelude//cxx:cuda.bzl",
    "CudaCompileStyle",
)
load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load(
    "@prelude//cxx:cxx_library.bzl",
    "cxx_library_parameterized",
)
load(
    "@prelude//cxx:cxx_library_utility.bzl",
    "cxx_attr_deps",
)
load(
    "@prelude//cxx:cxx_sources.bzl",
    "get_srcs_with_flags",
)
load(
    "@prelude//cxx:cxx_types.bzl",
    "CxxRuleConstructorParams",
    "CxxRuleProviderParams",
    "CxxRuleSubTargetParams",
)
load("@prelude//cxx:cxx_utility.bzl", "cxx_attrs_get_allow_cache_upload")
load("@prelude//cxx:headers.bzl", "CPrecompiledHeaderInfo", "cxx_get_regular_cxx_headers_layout")
load("@prelude//cxx:linker.bzl", "DUMPBIN_SUB_TARGET", "PDB_SUB_TARGET", "get_dumpbin_providers", "get_pdb_providers")
load(
    "@prelude//cxx:omnibus.bzl",
    "create_linkable_root",
    "get_roots",
)
load("@prelude//decls:common.bzl", "CxxRuntimeType", "CxxSourceType", "HeadersAsRawHeadersMode", "LinkableDepType", "buck")
load("@prelude//decls:native_common.bzl", "native_common")
load("@prelude//decls:third_party_common.bzl", "third_party_common")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load(
    "@prelude//linking:link_groups.bzl",
    "merge_link_group_lib_info",
)
load(
    "@prelude//linking:link_info.bzl", "LibOutputStyle", "LinkInfo", "LinkInfos", "LinkOrdering", "LinkableFlavor", "create_merged_link_info", "wrap_link_infos"
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "LinkableGraph",  # @unused Used as a type
    "create_linkable_graph",
    "create_linkable_graph_node",
    "create_linkable_node",
)
load(
    "@prelude//linking:linkables.bzl",
    "LinkableProviders",
    "linkables",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
    "merge_shared_libraries",
)
load("@prelude//linking:types.bzl", "Linkage")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")
load("@prelude//python:attrs.bzl", "python_common")
load("@prelude//python:toolchain.bzl", "PythonToolchainInfo")
load(
    "@prelude//python/linking:native_python_util.bzl",
    "merge_cxx_extension_info",
    "rewrite_static_symbols",
)
load(
    "@prelude//third-party:build.bzl",
    "create_third_party_build_root",
    "prefix_from_label",
)
load("@prelude//third-party:providers.bzl", "ThirdPartyBuild", "third_party_build_info")
load("@prelude//unix:providers.bzl", "UnixEnv", "create_unix_env_info")
load("@prelude//utils:utils.bzl", "value_or")
load(":manifest.bzl", "create_manifest_for_source_map")
load(":python.bzl", "NativeDepsInfo", "NativeDepsInfoTSet", "PythonLibraryInfo")
load(":python_library.bzl", "create_python_library_info", "dest_prefix", "gather_dep_libraries", "qualify_srcs")
load(":source_db.bzl", "create_python_source_db_info", "create_source_db_no_deps")
load(":versions.bzl", "gather_versioned_dependencies")

# This extension is basically cxx_library, plus base_module.
# So we augment with default attributes so it has everything cxx_library has, and then call cxx_library_parameterized and work from that.
def cxx_python_extension_impl(ctx: AnalysisContext) -> list[Provider]:
    providers = []

    if ctx.attrs._target_os_type[OsLookup].os == Os("windows"):
        library_extension = ".pyd"
    else:
        library_extension = ".so"
    module_name = value_or(ctx.attrs.module_name, ctx.label.name)
    name = module_name + library_extension
    base_module = dest_prefix(ctx.label, ctx.attrs.base_module)

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
        default = False,  # We need to do some postprocessing to make sure the shared library is our default output
        java_packaging_info = False,
        java_global_code_info = False,
        linkable_graph = False,  # We create this here so we can correctly apply exclusions
        link_style_outputs = False,
        merged_native_link_info = False,
        omnibus_root = True,
        preprocessors = False,
        resources = True,
        shared_libraries = False,
        template_placeholders = False,
        preprocessor_for_tests = False,
    )

    cxx_toolchain = get_cxx_toolchain_info(ctx)
    python_toolchain = ctx.attrs._python_toolchain[PythonToolchainInfo]

    impl_params = CxxRuleConstructorParams(
        build_empty_so = True,
        rule_type = "cxx_python_extension",
        headers_layout = cxx_get_regular_cxx_headers_layout(ctx),
        srcs = get_srcs_with_flags(ctx),
        soname = name,
        use_soname = False,
        generate_providers = cxx_providers,
        generate_sub_targets = sub_targets,
        compiler_flags = ctx.attrs.compiler_flags,
        lang_compiler_flags = ctx.attrs.lang_compiler_flags,
        extra_link_flags = python_toolchain.extension_linker_flags,
        preprocessor_flags = ctx.attrs.preprocessor_flags,
        lang_preprocessor_flags = ctx.attrs.lang_preprocessor_flags,
        error_handler = cxx_toolchain.cxx_error_handler,
        allow_cache_upload = cxx_attrs_get_allow_cache_upload(ctx.attrs, get_cxx_toolchain_info(ctx).cxx_compiler_info.allow_cache_upload),
        precompiled_header = ctx.attrs.precompiled_header,
        prefix_header = ctx.attrs.prefix_header,
        _cxx_toolchain = ctx.attrs._cxx_toolchain,
        coverage_instrumentation_compiler_flags = ctx.attrs.coverage_instrumentation_compiler_flags,
        coverage_profile_list = ctx.attrs.coverage_profile_list[DefaultInfo].default_outputs[0] if ctx.attrs.coverage_profile_list else None,
        separate_debug_info = ctx.attrs.separate_debug_info,
        cuda_compile_style = CudaCompileStyle(ctx.attrs.cuda_compile_style),
        supports_stripping = ctx.attrs.supports_stripping,
        use_content_based_paths = cxx_toolchain.cxx_compiler_info.supports_content_based_paths,
    )

    cxx_library_info = cxx_library_parameterized(ctx, impl_params)
    libraries = cxx_library_info.all_outputs
    shared_output = libraries.outputs[LibOutputStyle("shared_lib")][LinkableFlavor("default")]

    solib_default = libraries.solibs.get(LinkableFlavor("default"), None)
    if not solib_default:
        fail("Expected cxx_python_extension to produce a solib: {}".format(ctx.label))
    extension = solib_default.linked_object

    sub_targets = cxx_library_info.sub_targets
    if extension.pdb:
        sub_targets[PDB_SUB_TARGET] = get_pdb_providers(pdb = extension.pdb, binary = extension.output)

    dumpbin_toolchain_path = cxx_toolchain.dumpbin_toolchain_path
    if dumpbin_toolchain_path:
        sub_targets[DUMPBIN_SUB_TARGET] = get_dumpbin_providers(ctx, extension.output, dumpbin_toolchain_path)

    cxx_deps = cxx_attr_deps(ctx)

    extension_artifacts = {}
    python_module_names = {}

    link_infos = libraries.link_infos

    # For python_cxx_extensions we need to mangle the symbol names in order to avoid collisions
    # when linking into the main binary
    embeddable = ctx.attrs.allow_embedding and LibOutputStyle("archive") in libraries.outputs
    if embeddable:
        pyinit_prefix = "PyInit"
        if ctx.attrs._target_os_type[OsLookup].os == Os("macos"):
            pyinit_prefix = "_PyInit"
        if not ctx.attrs.allow_suffixing:
            pyinit_symbol = "{}_{}".format(pyinit_prefix, module_name)
        else:
            suffix = base_module.replace("/", "$") + module_name
            static_output = libraries.outputs[LibOutputStyle("archive")][LinkableFlavor("default")]
            static_pic_output = libraries.outputs[LibOutputStyle("pic_archive")][LinkableFlavor("default")]

            debuggable_static_pic_objects = []
            if LinkableFlavor("debug") in libraries.outputs[LibOutputStyle("pic_archive")]:
                debuggable_static_pic_objects = libraries.outputs[LibOutputStyle("pic_archive")][LinkableFlavor("debug")].object_files

            link_infos = rewrite_static_symbols(
                ctx,
                suffix,
                pic_objects = static_pic_output.object_files,
                non_pic_objects = static_output.object_files,
                debuggable_pic_objects = debuggable_static_pic_objects,
                libraries = link_infos,
                cxx_toolchain = cxx_toolchain,
                suffix_all = ctx.attrs.suffix_all,
                suffix_exclude_rtti = ctx.attrs.suffix_exclude_rtti,
            )
            pyinit_symbol = "{}_{}_{}".format(pyinit_prefix, module_name, suffix)

        if base_module != "":
            lines = ["# auto generated stub for {}\n".format(ctx.label.raw_target())]
            stub_name = module_name + ".empty_stub"
            extension_artifacts.update(
                qualify_srcs(ctx.label, ctx.attrs.base_module, {stub_name: ctx.actions.write(stub_name, lines, has_content_based_path = False)})
            )

        python_module_names[base_module.replace("/", ".") + module_name] = pyinit_symbol

    # A shared placeholder keeps this node eligible for all native link strategies.
    link_infos[LibOutputStyle("shared_lib")] = LinkInfos(default = LinkInfo())
    link_deps = linkables(cxx_deps)
    python_dep_libraries, shared_deps = gather_dep_libraries(ctx.attrs.deps, resolve_versioned_deps = False)
    linkable_providers = LinkableProviders(
        link_group_lib_info = merge_link_group_lib_info(deps = cxx_deps),
        linkable_graph = create_linkable_graph(
            ctx = ctx,
            node = create_linkable_graph_node(
                ctx = ctx,
                linkable_node = create_linkable_node(
                    ctx = ctx,
                    deps = cxx_deps,
                    preferred_linkage = Linkage("any"),
                    link_infos = link_infos,
                    default_soname = name,
                ),
            ),
            deps = [d.linkable_graph for d in link_deps if d.linkable_graph != None],
        ),
        merged_link_info = create_merged_link_info(
            ctx = ctx,
            pic_behavior = cxx_toolchain.pic_behavior,
            link_infos = link_infos,
            preferred_linkage = Linkage("static"),
            deps = [d.merged_link_info for d in link_deps],
        ),
        shared_library_info = merge_shared_libraries(
            actions = ctx.actions,
            deps = [d.shared_library_info for d in link_deps],
        ),
        linkable_root_info = create_linkable_root(
            label = ctx.label,
            link_infos = wrap_link_infos(
                link_infos[LibOutputStyle("pic_archive")],
                pre_flags = ctx.attrs.linker_flags,
                post_flags = ctx.attrs.post_linker_flags,
            ),
            deps = cxx_deps,
        ),
    )
    omnibus_graph = create_linkable_graph(
        ctx,
        node = create_linkable_graph_node(
            ctx,
            roots = get_roots([dep for dep in ctx.attrs.deps if PythonLibraryInfo in dep]),
        ),
        deps = ctx.attrs.deps,
    )

    providers.extend(cxx_library_info.providers)
    providers.extend(
        python_extension_providers(
            ctx = ctx,
            extension = extension,
            shared_output = shared_output,
            linkable_providers = linkable_providers,
            omnibus_graph = omnibus_graph,
            extension_shared_libraries = shared_deps,
            extension_deps = cxx_deps,
            raw_deps = ctx.attrs.deps,
            python_dep_libraries = python_dep_libraries,
            soname = name,
            base_module = base_module,
            base_module_attr = ctx.attrs.base_module,
            module_name = module_name,
            type_stub = ctx.attrs.type_stub,
            embeddable = embeddable,
            extension_artifacts = extension_artifacts,
            python_module_names = python_module_names,
            sub_targets = sub_targets,
        )
    )
    return providers

def python_extension_providers(
    *,
    ctx: AnalysisContext,
    extension,
    shared_output,
    linkable_providers: LinkableProviders,
    omnibus_graph: LinkableGraph,
    extension_shared_libraries: list[SharedLibraryInfo],
    extension_deps: list[Dependency],
    raw_deps: list[Dependency],
    python_dep_libraries: list[PythonLibraryInfo],
    soname: str,
    base_module: str,
    base_module_attr,
    module_name: str,
    type_stub,
    embeddable: bool,
    extension_artifacts: dict,
    python_module_names: dict,
    sub_targets: dict,
) -> list[Provider]:
    providers = []
    unembeddable_extensions = {}
    if not embeddable:
        unembeddable_extensions[base_module + soname] = linkable_providers

    providers.append(
        merge_cxx_extension_info(
            actions = ctx.actions,
            deps = extension_deps,
            linkable_providers = linkable_providers if embeddable else None,
            artifacts = extension_artifacts,
            python_module_names = python_module_names,
            unembeddable_extensions = unembeddable_extensions,
        )
    )

    # If a type stub was specified, create a manifest for export.
    src_types = None
    src_type_manifest = None
    if type_stub != None:
        src_types = qualify_srcs(
            ctx.label,
            base_module_attr,
            {module_name + ".pyi": type_stub},
        )
        src_type_manifest = create_manifest_for_source_map(
            ctx,
            "type_stub",
            src_types,
        )

    providers.append(gather_versioned_dependencies(raw_deps))

    # We dont process anything for python extensions, we just add an empty set.
    native_deps = ctx.actions.tset(
        NativeDepsInfoTSet,
        value = NativeDepsInfo(native_deps = {}),
        children = [],
    )
    library_info = create_python_library_info(
        ctx.actions,
        ctx.label,
        extensions = qualify_srcs(ctx.label, base_module_attr, {soname: extension}),
        deps = python_dep_libraries,
        extension_shared_libraries = extension_shared_libraries,
        src_types = src_type_manifest,
        native_deps = native_deps,
        is_native_dep = True,
    )
    providers.append(library_info)

    # Source DBs.
    if src_types != None:
        sub_targets["source-db-no-deps"] = [create_source_db_no_deps(ctx, src_types), create_python_source_db_info(library_info.manifests)]

    providers.append(
        DefaultInfo(
            default_output = shared_output.default,
            other_outputs = shared_output.other,
            sub_targets = sub_targets,
        )
    )

    providers.append(omnibus_graph)

    # Allow third-party-build rules to depend on Python rules.
    tp_prefix = prefix_from_label(ctx.label)
    providers.append(
        third_party_build_info(
            actions = ctx.actions,
            build = ThirdPartyBuild(
                prefix = tp_prefix,
                root = create_third_party_build_root(
                    ctx = ctx,
                    paths = [(paths.join("lib/python", base_module + soname), extension.output)],
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

    return providers

cxx_python_extension = prelude_rule(
    name = "cxx_python_extension",
    impl = cxx_python_extension_impl,
    docs = """
        A `cxx_python_extension()` rule is a variant of a C/C++ library which is built as a Python module. As such,
        it has a module name formed by the `base_module` parameter and the rule name.
    """,
    examples = """
        ```
        # A rule that builds a Python extension from a single .cpp file.
        cxx_python_extension(
          name = 'mymodule',
          base_module = 'foo.bar',
          srcs = [
            'mymodule.cpp',
          ],
        )

        # A library rule which has a single source importing the above extension.
        python_library(
          name = 'utils',
          srcs = [
            'utils.py',
          ],
          deps = [
            ':mymodule',
          ],
        )
        ```

        ```
        ## The `utils.py` source, wrapped by the `utils` rule above.

        ## Import the C/C++ extension build above.
        from foo.bar import mymodule

        ...
        ```
    """,
    further = None,
    attrs = (
        # cxx_python_extension is a subset of cxx_library, plus a base_module.
        # So we can reuse cxx_library, we augment it with the additional attributes it defines.
        # This isn't the ideal way to reuse it (we'd rather cxx_library was split it multiple reusable parts),
        # but it's the pragmatic way of getting it working for now.
        # @unsorted-dict-items
        {k: attrs.default_only(v) for k, v in cxx_library_base_attrs().items()}
        | buck.labels_arg()
        | python_common.base_module_arg()
        | cxx_common.srcs_arg()
        | cxx_common.deps_arg()
        | cxx_common.headers_arg()
        | cxx_common.header_namespace_arg()
        | cxx_common.preprocessor_flags_arg()
        | cxx_common.compiler_flags_arg()
        | {
            "link_style": attrs.option(
                attrs.enum(LinkableDepType),
                default = None,
                doc = """
                Determines whether to build and link this rule's dependencies statically or dynamically.
                 Can be either `static`, `static_pic` or `shared`.
                 Note: since shared libraries re-export its dependencies, depending on multiple shared libraries
                 which themselves have overlapping static dependencies may cause problems if they init using global state.
            """,
            ),
        }
        | cxx_common.linker_extra_outputs_arg()
        | cxx_common.linker_flags_arg()
        | cxx_common.local_linker_flags_arg()
        | cxx_common.supports_stripping()
        | cxx_common.default_deps_arg()
        | native_common.transformation_spec_arg()
        | third_party_common.create_third_party_build_root_attrs()
        | {
            "cxx_runtime_type": attrs.option(attrs.enum(CxxRuntimeType), default = None),
            "default_platform": attrs.option(attrs.string(), default = None),
            "defaults": attrs.dict(key = attrs.string(), value = attrs.string(), sorted = False, default = {}),
            "executable_name": attrs.option(attrs.string(), default = None),
            "frameworks": attrs.list(attrs.string(), default = []),
            "headers_as_raw_headers_mode": attrs.option(attrs.enum(HeadersAsRawHeadersMode), default = None),
            "include_directories": attrs.set(attrs.string(), sorted = True, default = []),
            "lang_compiler_flags": attrs.dict(key = attrs.enum(CxxSourceType), value = attrs.list(attrs.arg()), sorted = False, default = {}),
            "lang_preprocessor_flags": attrs.dict(key = attrs.enum(CxxSourceType), value = attrs.list(attrs.arg()), sorted = False, default = {}),
            "libraries": attrs.list(attrs.string(), default = []),
            "module_name": attrs.option(attrs.string(), default = None),
            "post_linker_flags": attrs.list(attrs.arg(anon_target_compatible = True), default = []),
            "precompiled_header": attrs.option(attrs.source(), default = None),
            "prefix_header": attrs.option(attrs.source(), default = None),
            "raw_headers": attrs.set(attrs.source(), sorted = True, default = []),
            "type_stub": attrs.option(attrs.source(), default = None),
        }
        | {
            "allow_embedding": attrs.bool(default = True),
            "allow_suffixing": attrs.bool(default = True),
            # Copied from cxx_library.
            "auto_link_groups": attrs.bool(default = False),
            # These flags will only be used to instrument a target
            # when coverage for that target is enabled by `exported_needs_coverage_instrumentation`
            # or by any of the target's dependencies.
            "coverage_instrumentation_compiler_flags": attrs.list(attrs.string(), default = []),
            # Optional clang_profile_list target for selective coverage instrumentation via -fprofile-list.
            "coverage_profile_list": attrs.option(attrs.dep(), default = None),
            "cuda_compile_style": attrs.enum(CudaCompileStyle.values(), default = "mono"),
            "exported_needs_coverage_instrumentation": attrs.bool(default = False),
            "extra_dwp_flags": attrs.list(attrs.string(), default = []),
            "link_ordering": attrs.option(attrs.enum(LinkOrdering.values()), default = None),
            "link_whole": attrs.default_only(attrs.bool(default = True)),
            "precompiled_header": attrs.option(attrs.dep(providers = [CPrecompiledHeaderInfo]), default = None),
            "preferred_linkage": attrs.default_only(attrs.string(default = "any")),
            "separate_debug_info": attrs.bool(default = False),
            "suffix_all": attrs.bool(default = True),
            "suffix_exclude_rtti": attrs.bool(default = False),
            "support_shlib_interfaces": attrs.bool(default = True),
            "_cxx_hacks": attrs.default_only(attrs.dep(default = "prelude//cxx/tools:cxx_hacks")),
            "_cxx_toolchain": toolchains_common.cxx(),
            # Copied from python_library.
            "_python_internal_tools": python_common.internal_tools_arg(),
            "_python_toolchain": toolchains_common.python(),
            "_target_os_type": buck.target_os_type_arg(),
        }
        | buck.licenses_arg()
        | buck.contacts_arg()
    ),
)
