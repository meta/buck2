# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//cxx:cxx_context.bzl", "get_opt_cxx_toolchain_info")
load("@prelude//cxx:headers.bzl", "CxxHeadersLayout", "CxxHeadersNaming")
load("@prelude//cxx:target_sdk_version.bzl", "get_target_sdk_version_flags")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//go/transitions:defs.bzl", "build_tags_attr", "cgo_enabled_attr", "go_stdlib_transition")
load("@prelude//os_lookup:defs.bzl", "OsLookup")
load("@prelude//utils:graph_utils.bzl", "post_order_traversal")
load(
    ":cgo_builder.bzl",
    "CGoBuildContext",  # @Unused used as type
)
load(":go_list_stdlib.bzl", "go_list_stdlib", "parse_go_list_stdlib_out")
load(":package_builder.bzl", "BuildPackageGoList", "BuildPackageParams", "build_package", "go_list_for_build")
load(":packages.bzl", "GoPkg", "GoStdlib", "GoStdlibDynamicValue", "implicit_imports")
load(":toolchain.bzl", "GoToolchainInfo", "evaluate_cgo_enabled")

def go_stdlib_impl(ctx: AnalysisContext) -> list[Provider]:
    go_toolchain = ctx.attrs._go_toolchain[GoToolchainInfo]

    cgo_enabled = evaluate_cgo_enabled(ctx.attrs._cgo_enabled)

    go_list_stdlib_out = go_list_stdlib(ctx.actions, go_toolchain, cgo_enabled)

    pkgdir = ctx.actions.declare_output("pkgdir", dir = True, has_content_based_path = True)

    go_stdlib_value = ctx.actions.dynamic_output_new(
        _build_stdlib(
            goroot = go_toolchain.env_go_root,
            go_list_stdlib_out = go_list_stdlib_out,
            target_label = ctx.label,
            go_toolchain = go_toolchain,
            cgo_build_context = get_cgo_build_context(ctx),
            pkgdir = pkgdir.as_output(),
        )
    )

    return [
        DefaultInfo(default_output = pkgdir),
        GoStdlib(dynamic_value = go_stdlib_value),
    ]

def _build_stdlib_impl(
    actions: AnalysisActions,
    target_label: Label,
    go_toolchain: GoToolchainInfo,
    cgo_build_context: None | CGoBuildContext,
    go_list_stdlib_out: ArtifactValue,
    goroot: Artifact,
    pkgdir: OutputArtifact,
) -> list[Provider]:
    # First pass: parse all packages and collect their imports
    parsed_libs = {}  # import_path -> StdGoListOut
    for lib in go_list_stdlib_out.read_json():
        lib = parse_go_list_stdlib_out(goroot, lib)
        if len(lib.go_list.go_files) == 0 and len(lib.go_list.cgo_files) == 0:
            continue  # skip test only packages

        if lib.import_path in ["unsafe", "builtin"]:
            continue  # skip fake packages

        parsed_libs[lib.import_path] = lib

    # Build dependency graph: import_path -> [imports that are also in stdlib]
    dep_graph = {}
    for import_path, lib in parsed_libs.items():
        dep_graph[import_path] = []

        all_imports = set(lib.go_list.imports) | implicit_imports(
            pkg_name = lib.go_list.name,
            pkg_import_path = import_path,
            standard = True,
            has_cgo_files = len(lib.go_list.cgo_files) > 0,
            coverage_enabled = False,
            coverage_mode = None,
        )
        for imp in all_imports:
            if imp in parsed_libs:
                dep_graph[import_path].append(imp)

    # Topological sort: post_order_traversal returns leaves (no deps) first,
    # giving us the correct build order (dependencies before dependents)
    build_order = post_order_traversal(dep_graph)

    # Second pass: build packages in dependency order
    pkgs = {}
    for import_path in build_order:
        lib = parsed_libs[import_path]

        pkgs[lib.import_path] = _declare_stdlib_package_build(
            actions = actions,
            target_label = target_label,
            go_toolchain = go_toolchain,
            cgo_build_context = cgo_build_context,
            go_list = go_list_for_build(lib.go_list, False),
            params = BuildPackageParams(
                main = False,
                standard = True,
                pkg_import_path = lib.import_path,
                package_root = goroot.short_path + "/src/" + lib.import_path,
                embed_srcs = {src.short_path: src for src in lib.embed_files},
                compiler_flags = [],
                assembler_flags = [],
                coverage_enabled = False,
                coverage_mode = None,  # we can switch-on stdlib coverage later if needed
                deps = pkgs,
                import_map = lib.import_map,
            ),
        )

    # We use pkgdir artifact testing (and currently for mockgen), but not for compilation/linking
    actions.copied_dir(
        pkgdir,
        {path + ".a": pkg.archive_file for path, pkg in pkgs.items()} | {path + ".x": pkg.export_file for path, pkg in pkgs.items()},
    )

    return [
        GoStdlibDynamicValue(pkgs = pkgs),
    ]

_build_stdlib = dynamic_actions(
    impl = _build_stdlib_impl,
    attrs = {
        "cgo_build_context": dynattrs.value(None | CGoBuildContext),
        "go_list_stdlib_out": dynattrs.artifact_value(),
        "go_toolchain": dynattrs.value(GoToolchainInfo),
        "goroot": dynattrs.value(Artifact),
        "pkgdir": dynattrs.output(),
        "target_label": dynattrs.value(Label),
    },
)

def _declare_stdlib_package_build(
    actions: AnalysisActions,
    target_label: Label,
    go_toolchain: GoToolchainInfo,
    cgo_build_context: None | CGoBuildContext,
    go_list: BuildPackageGoList,
    params: BuildPackageParams,
) -> GoPkg:
    out_a = actions.declare_output(params.pkg_import_path + "_non-shared.a", has_content_based_path = True)
    out_x = actions.declare_output(params.pkg_import_path + "_non-shared.x", has_content_based_path = True)

    out_a_shared = actions.declare_output(params.pkg_import_path + "_shared.a", has_content_based_path = True)
    out_x_shared = actions.declare_output(params.pkg_import_path + "_shared.x", has_content_based_path = True)

    actions.dynamic_output_new(
        _build_stdlib_package(
            target_label = target_label,
            go_toolchain = go_toolchain,
            cgo_build_context = cgo_build_context,
            go_list = go_list,
            params = params,
            out_a = out_a.as_output(),
            out_x = out_x.as_output(),
            out_a_shared = out_a_shared.as_output(),
            out_x_shared = out_x_shared.as_output(),
        )
    )

    return GoPkg(
        archive_file = out_a,
        archive_file_shared = out_a_shared,
        export_file = out_x,
        export_file_shared = out_x_shared,
        coverage_instrumented = params.coverage_enabled and params.coverage_mode != None,
    )

def _build_stdlib_package_impl(
    actions: AnalysisActions,
    target_label: Label,
    go_toolchain: GoToolchainInfo,
    cgo_build_context: None | CGoBuildContext,
    go_list: BuildPackageGoList,
    params: BuildPackageParams,
    out_a: OutputArtifact,
    out_x: OutputArtifact,
    out_a_shared: OutputArtifact,
    out_x_shared: OutputArtifact,
) -> list[Provider]:
    result = build_package(
        actions = actions,
        target_label = target_label,
        go_toolchain = go_toolchain,
        cgo_build_context = cgo_build_context,
        go_list = go_list,
        params = params,
    )
    actions.copy_file(out_a, result.a_file)
    actions.copy_file(out_x, result.x_file)
    actions.copy_file(out_a_shared, result.a_file_shared)
    actions.copy_file(out_x_shared, result.x_file_shared)
    return []

_build_stdlib_package = dynamic_actions(
    impl = _build_stdlib_package_impl,
    # @unsorted-dict-items
    attrs = {
        # Inputs
        "target_label": dynattrs.value(Label),
        "go_toolchain": dynattrs.value(GoToolchainInfo),
        "cgo_build_context": dynattrs.value(None | CGoBuildContext),
        "go_list": dynattrs.value(BuildPackageGoList),
        "params": dynattrs.value(BuildPackageParams),
        # Outputs
        "out_a": dynattrs.output(),
        "out_x": dynattrs.output(),
        "out_a_shared": dynattrs.output(),
        "out_x_shared": dynattrs.output(),
    },
)

def get_cgo_build_context(ctx: AnalysisContext) -> CGoBuildContext | None:
    cxx_toolchain_info = get_opt_cxx_toolchain_info(ctx)
    if cxx_toolchain_info == None:
        return None

    return CGoBuildContext(
        cxx_toolchain_info = cxx_toolchain_info,
        target_sdk_version_flags = get_target_sdk_version_flags(ctx),
        exec_os_type = ctx.attrs._exec_os_type[OsLookup],
        header_namespace = "",
        headers_layout = CxxHeadersLayout(namespace = "", naming = CxxHeadersNaming("regular")),
        cxx_compiler_flags = [],
        cxx_preprocessor_flags = [],
        inherited_preprocessor_infos = [],
        _cxx_toolchain = ctx.attrs._cxx_toolchain,
    )

go_stdlib = prelude_rule(
    name = "go_stdlib",
    impl = go_stdlib_impl,
    docs = """
        A `go_stdlib()` rule represents the Go standard library for the
        target configuration. It has no user-facing attributes and is
        consumed implicitly by other Go rules.
    """,
    attrs = (
        {}
        | {
            "_build_tags": build_tags_attr,
            "_cgo_enabled": cgo_enabled_attr,
            "_cxx_toolchain": toolchains_common.cxx(),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_go_toolchain": toolchains_common.go(),
        }
    ),
    cfg = go_stdlib_transition,
)
