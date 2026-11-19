# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//cxx:attrs.bzl", "BUILD_INFO_ATTR", "cxx_common")
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxToolchainInfo")
load("@prelude//cxx:linker.bzl", "get_default_shared_library_name")
load(
    "@prelude//cxx:preprocessor.bzl",
    "cxx_inherited_preprocessor_infos",
    "cxx_merge_cpreprocessors",
)
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//go/transitions:defs.bzl", "build_tags_attr", "cgo_enabled_attr", "go_exported_library_transition")
load(
    "@prelude//linking:link_groups.bzl",
    "merge_link_group_lib_info",
)
load(
    "@prelude//linking:link_info.bzl",
    "Archive",
    "ArchiveLinkable",
    "LibOutputStyle",
    "LinkInfo",
    "LinkInfos",
    "LinkStyle",
    "LinkedObject",
    "MergedLinkInfo",  # @unused Used as a type
    "SharedLibLinkable",
    "create_merged_link_info",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "create_linkable_graph",
    "create_linkable_graph_node",
    "create_linkable_node",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraries",
    "SharedLibraryInfo",
    "create_shlib",
    "merge_shared_libraries",
)
load(
    "@prelude//utils:utils.bzl",
    "from_named_set",
    "map_idx",
    "map_val",
    "value_or",
)
load(":attrs.bzl", "BuildMode", "go_common")
load(":cgo_builder.bzl", "get_cgo_build_context")
load(":compile.bzl", "GoTestInfo")
load(":link.bzl", "GoBuildMode", "link")
load(":package_builder.bzl", "GoBuildConfig", "GoSourceInputs", "declare_package_build")
load(":packages.bzl", "cgo_exported_preprocessor", "go_attr_pkg_name")
load(":toolchain.bzl", "evaluate_cgo_enabled")

def go_exported_library_impl(ctx: AnalysisContext) -> list[Provider]:
    pkg_import_path = go_attr_pkg_name(ctx)
    cgo_enabled = evaluate_cgo_enabled(ctx.attrs._cgo_enabled)
    cgo_build_context = get_cgo_build_context(ctx)

    lib, pkg_info, _ = declare_package_build(
        ctx = ctx,
        pkg_import_path = pkg_import_path,
        main = True,
        sources = GoSourceInputs(
            srcs = ctx.attrs.srcs,
            embed_srcs = from_named_set(ctx.attrs.embed_srcs),
            package_root = ctx.attrs.package_root,
        ),
        cgo_build_context = cgo_build_context,
        config = GoBuildConfig(
            compiler_flags = ctx.attrs.compiler_flags,
            build_tags = ctx.attrs._build_tags,
            cgo_enabled = cgo_enabled,
        ),
        deps = ctx.attrs.deps,
    )

    def link_variant(build_mode: GoBuildMode):
        (exp_lib, _, _) = link(
            ctx,
            lib,
            cgo_enabled = cgo_enabled,
            deps = ctx.attrs.deps,
            build_mode = build_mode,
            link_style = value_or(map_val(LinkStyle, ctx.attrs.link_style), LinkStyle("static_pic")),
            linker_flags = ctx.attrs.linker_flags,
            external_linker_flags = ctx.attrs.external_linker_flags,
        )
        return exp_lib

    c_archive = link_variant(GoBuildMode("c_archive"))  # .a - PIC-arcive
    c_shared = link_variant(GoBuildMode("c_shared"))  # .so - PIC-shared_lib

    cxx_toolchain = ctx.attrs._cxx_toolchain[CxxToolchainInfo]

    soname = get_default_shared_library_name(cxx_toolchain.linker_info, ctx.label)

    link_infos = {
        LibOutputStyle("archive"): LinkInfos(
            default = LinkInfo(
                linkables = [
                    ArchiveLinkable(
                        archive = Archive(artifact = c_archive),
                        linker_type = cxx_toolchain.linker_info.type,
                    )
                ]
            ),
        ),
        LibOutputStyle("pic_archive"): LinkInfos(
            default = LinkInfo(
                linkables = [
                    ArchiveLinkable(
                        archive = Archive(artifact = c_archive),
                        linker_type = cxx_toolchain.linker_info.type,
                    )
                ]
            ),
        ),
        LibOutputStyle("shared_lib"): LinkInfos(
            default = LinkInfo(
                linkables = [
                    SharedLibLinkable(
                        lib = c_shared,
                    )
                ]
            ),
        ),
    }

    shared_libs = SharedLibraries(
        libraries = [
            create_shlib(
                soname = soname,
                label = ctx.label,
                lib = LinkedObject(
                    output = c_shared,
                    unstripped_output = c_shared,
                ),
            ),
        ]
    )

    own_exported_preprocessors = [cgo_exported_preprocessor(ctx, pkg_info)] if ctx.attrs.generate_exported_header else []

    return [
        DefaultInfo(
            default_output = c_archive if ctx.attrs.build_mode == "c_archive" else c_shared,
        ),
        GoTestInfo(
            deps = ctx.attrs.deps,
            srcs = ctx.attrs.srcs,
            pkg_import_path = pkg_import_path,
            coverage_enabled = ctx.attrs.coverage_enabled,
        ),
        create_merged_link_info(
            ctx,
            cxx_toolchain.pic_behavior,
            link_infos = link_infos,
            deps = filter(None, map_idx(MergedLinkInfo, ctx.attrs.deps)),
        ),
        merge_shared_libraries(
            ctx.actions,
            node = shared_libs,
            deps = filter(None, map_idx(SharedLibraryInfo, ctx.attrs.deps)),
        ),
        merge_link_group_lib_info(deps = ctx.attrs.deps),
        create_linkable_graph(
            ctx,
            node = create_linkable_graph_node(
                ctx,
                linkable_node = create_linkable_node(
                    ctx,
                    default_soname = soname,
                    deps = ctx.attrs.deps,
                    link_infos = link_infos,
                    shared_libs = shared_libs,
                ),
            ),
            deps = ctx.attrs.deps,
        ),
        cxx_merge_cpreprocessors(ctx.actions, own_exported_preprocessors, cxx_inherited_preprocessor_infos(ctx.attrs.deps)),
        pkg_info,
    ]

go_exported_library = prelude_rule(
    name = "go_exported_library",
    impl = go_exported_library_impl,
    docs = """
        A go\\_exported\\_library() rule builds a C library from the supplied set of Go source files
         and dependencies. This is done via `-buildmode` flag and "//export" annotations in the code.
    """,
    examples = """
        For more examples, check out our [integration tests](https://github.com/facebook/buck/tree/dev/test/com/facebook/buck/features/go/testdata).

        ```
        go_exported_library(
            name = "shared",
            srcs = ["main.go"],
            build_mode = "c_shared",
            compiler_flags = ["-shared"],
            deps = [":example"],
        )

        go_library(
            name = "example",
            package_name = "cgo",
            srcs = [
                "export-to-c.go",  # file with //export annotations
            ],
            compiler_flags = [],
            headers = [],
        )

        cxx_genrule(
            name = "cgo_exported_headers",
            out = "includes",
            cmd = (
                "mkdir -p $OUT && " +
                "cat `dirname $(location :shared)`/includes/*.h > $OUT/_cgo_export.h"
            ),
        )

        prebuilt_cxx_library(
            name = "cxx_so_with_header",
            header_dirs = [":cgo_exported_headers"],
            shared_lib = ":shared",
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        go_common.package_name_arg()
        | go_common.srcs_arg()
        | go_common.deps_arg()
        | {
            "build_mode": attrs.option(
                attrs.enum(BuildMode),
                doc = """
                Determines the build mode (equivalent of `-buildmode`). Can be
                 one of the following values: `c_archive`, `c_shared`.`
            """,
                default = None,
            ),
        }
        | cxx_common.headers_arg()
        | cxx_common.header_namespace_arg()
        | go_common.cxx_preprocessor_flags_arg()
        | go_common.cxx_compiler_flags_arg()
        | go_common.link_style_arg()
        | go_common.link_mode_arg()
        | go_common.compiler_flags_arg()
        | go_common.assembler_flags_arg()
        | go_common.linker_flags_arg()
        | go_common.external_linker_flags_arg()
        | go_common.package_root_arg()
        | go_common.cgo_enabled_arg()
        | go_common.build_tags_arg()
        | go_common.generate_exported_header()
        | go_common.coverage_enabled_arg()
        | go_common.embed_srcs_arg()
        | {
            "platform": attrs.option(attrs.string(), default = None),
            "resources": attrs.list(
                attrs.source(),
                default = [],
                doc = """
                Static files to be symlinked into the working directory of the test. You can access these in your
                 by opening the files as relative paths, e.g. `ioutil.ReadFile("testdata/input")`.
            """,
            ),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | {
            "_build_info": BUILD_INFO_ATTR,
            "_build_tags": build_tags_attr,
            "_cgo_enabled": cgo_enabled_attr,
            "_cxx_toolchain": toolchains_common.cxx(),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_go_stdlib": attrs.default_only(attrs.dep(default = "prelude//go/tools:stdlib")),
            "_go_toolchain": toolchains_common.go(),
        }
    ),
    cfg = go_exported_library_transition,
)
