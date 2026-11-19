# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//cxx:attrs.bzl", "cxx_common")
load(
    "@prelude//cxx:preprocessor.bzl",
    "cxx_inherited_preprocessor_infos",
    "cxx_merge_cpreprocessors",
)
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//go/transitions:defs.bzl", "build_tags_attr", "cgo_enabled_attr", "coverage_mode_attr", "go_library_transition")
load(
    "@prelude//linking:link_groups.bzl",
    "merge_link_group_lib_info",
)
load(
    "@prelude//linking:link_info.bzl",
    "LibOutputStyle",
    "LinkInfo",
    "LinkInfos",
    "MergedLinkInfo",
    "create_merged_link_info_for_propagation",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "create_linkable_graph",
    "create_linkable_graph_node",
    "create_linkable_node",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
    "merge_shared_libraries",
)
load(
    "@prelude//utils:utils.bzl",
    "from_named_set",
    "map_idx",
)
load(":attrs.bzl", "go_common")
load(":cgo_builder.bzl", "get_cgo_build_context")
load(":compile.bzl", "GoPkgCompileInfo", "GoTestInfo")
load(":coverage.bzl", "GoCoverageMode")
load(":link.bzl", "GoPkgLinkInfo", "get_inherited_link_pkgs")
load(":package_builder.bzl", "GoBuildConfig", "GoSourceInputs", "declare_package_build")
load(":packages.bzl", "cgo_exported_preprocessor", "go_attr_pkg_name", "merge_pkgs")
load(":toolchain.bzl", "GoToolchainInfo", "evaluate_cgo_enabled", "get_toolchain_env_vars")

def go_library_impl(ctx: AnalysisContext) -> list[Provider]:
    pkg_import_path = go_attr_pkg_name(ctx)

    coverage_mode = GoCoverageMode(ctx.attrs._coverage_mode) if ctx.attrs._coverage_mode else None
    cgo_build_context = get_cgo_build_context(ctx)

    pkg, pkg_info, _ = declare_package_build(
        ctx = ctx,
        pkg_import_path = pkg_import_path,
        main = False,
        sources = GoSourceInputs(
            srcs = ctx.attrs.srcs + ctx.attrs.headers,
            embed_srcs = from_named_set(ctx.attrs.embed_srcs),
            package_root = ctx.attrs.package_root,
        ),
        cgo_build_context = cgo_build_context,
        config = GoBuildConfig(
            compiler_flags = ctx.attrs.compiler_flags,
            assembler_flags = ctx.attrs.assembler_flags,
            build_tags = ctx.attrs._build_tags,
            coverage_enabled = ctx.attrs.coverage_enabled,
            coverage_mode = coverage_mode,
            cgo_enabled = evaluate_cgo_enabled(ctx.attrs._cgo_enabled, ctx.attrs.override_cgo_enabled),
        ),
        deps = ctx.attrs.deps,
    )

    default_output = _combine_package(ctx, pkg_import_path, pkg.archive_file, pkg.export_file)
    pkgs = {
        pkg_import_path: pkg,
    }

    own_exported_preprocessors = [cgo_exported_preprocessor(ctx, pkg_info)] if ctx.attrs.generate_exported_header else []

    return [
        DefaultInfo(default_output = default_output),
        GoPkgCompileInfo(pkgs = pkgs),
        GoPkgLinkInfo(
            pkgs = merge_pkgs([
                pkgs,
                get_inherited_link_pkgs(ctx.attrs.deps),
            ])
        ),
        GoTestInfo(
            deps = ctx.attrs.deps,
            srcs = ctx.attrs.srcs,
            pkg_import_path = pkg_import_path,
            coverage_enabled = ctx.attrs.coverage_enabled,
        ),
        create_merged_link_info_for_propagation(ctx, filter(None, [d.get(MergedLinkInfo) for d in ctx.attrs.deps])),
        merge_shared_libraries(
            ctx.actions,
            deps = filter(None, map_idx(SharedLibraryInfo, ctx.attrs.deps)),
        ),
        merge_link_group_lib_info(deps = ctx.attrs.deps),
        create_linkable_graph(
            ctx,
            # Linkable graph nodes must be present for link_groups operation (even if there are nothing to provide).
            # Borrowing the this approach from Ocaml.
            # Though his doesn't look entirely right.
            node = create_linkable_graph_node(
                ctx,
                linkable_node = create_linkable_node(
                    ctx,
                    default_soname = None,
                    exported_deps = ctx.attrs.deps,
                    link_infos = _get_empty_link_infos(),
                ),
            ),
            deps = ctx.attrs.deps,
        ),
        cxx_merge_cpreprocessors(ctx.actions, own_exported_preprocessors, cxx_inherited_preprocessor_infos(ctx.attrs.deps)),
        pkg_info,
    ]

def _get_empty_link_infos() -> dict[LibOutputStyle, LinkInfos]:
    infos = {}
    for output_style in LibOutputStyle:
        infos[output_style] = LinkInfos(default = LinkInfo())
    return infos

# The combined package is convinient for debugging purposes, but for actual builds we use separate objects.
def _combine_package(ctx: AnalysisContext, pkg_import_path: str, a_file: Artifact, x_file: Artifact) -> Artifact:
    go_toolchain = ctx.attrs._go_toolchain[GoToolchainInfo]
    env = get_toolchain_env_vars(go_toolchain)

    pkg_file = ctx.actions.declare_output(paths.basename(pkg_import_path) + "-combined.a", has_content_based_path = True)

    pack_cmd = [
        go_toolchain.packer,
        "c",
        pkg_file.as_output(),
        a_file,
        x_file,
    ]

    identifier = paths.basename(pkg_import_path) + "-combined"
    ctx.actions.run(pack_cmd, env = env, category = "go_pack", identifier = identifier, allow_cache_upload = go_toolchain.allow_cache_upload)

    return pkg_file

go_library = prelude_rule(
    name = "go_library",
    impl = go_library_impl,
    docs = """
        A go\\_library() rule builds a native library from the supplied set of Go source files
         and dependencies.
    """,
    examples = """
        For more examples, check out our [integration tests](https://github.com/facebook/buck/tree/dev/test/com/facebook/buck/features/go/testdata).

        ```
        go_library(
          name='greeting',
          srcs=[
            'greeting.go',
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
        go_common.srcs_arg()
        | go_common.package_name_arg()
        | go_common.deps_arg()
        | go_common.compiler_flags_arg()
        | go_common.assembler_flags_arg()
        | go_common.embed_srcs_arg()
        | go_common.package_root_arg()
        | go_common.override_cgo_enabled_arg()
        | cxx_common.headers_arg()
        | cxx_common.header_namespace_arg()
        | go_common.cxx_preprocessor_flags_arg()
        | go_common.cxx_compiler_flags_arg()
        | go_common.external_linker_flags_arg()
        | go_common.link_style_arg()
        | go_common.coverage_enabled_arg()
        | go_common.generate_exported_header()
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | {
            "_build_tags": build_tags_attr,
            "_cgo_enabled": cgo_enabled_attr,
            "_coverage_mode": coverage_mode_attr,
            "_cxx_toolchain": toolchains_common.cxx(),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_go_stdlib": attrs.default_only(attrs.dep(default = "prelude//go/tools:stdlib")),
            "_go_toolchain": toolchains_common.go(),
        }
    ),
    cfg = go_library_transition,
)
