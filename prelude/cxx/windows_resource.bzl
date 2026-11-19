# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//cxx:attrs.bzl", "cxx_common")
load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load("@prelude//cxx:cxx_library_utility.bzl", "cxx_attr_deps")
load("@prelude//cxx:headers.bzl", "cxx_get_regular_cxx_headers_layout")
load("@prelude//cxx:preprocessor.bzl", "cxx_inherited_preprocessor_infos", "cxx_merge_cpreprocessors", "cxx_private_preprocessor_info")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//linking:link_groups.bzl", "EMPTY_LINK_GROUP_LIB_INFO")
load("@prelude//linking:link_info.bzl", "LibOutputStyle", "LinkInfo", "LinkInfos", "ObjectsLinkable", "create_merged_link_info")
load("@prelude//linking:linkable_graph.bzl", "create_linkable_graph")
load("@prelude//linking:shared_libraries.bzl", "EMPTY_SHARED_LIBRARY_INFO")

def windows_resource_impl(ctx: AnalysisContext) -> list[Provider]:
    non_exported_deps = cxx_attr_deps(ctx)

    (own_non_exported_preprocessor_info, _) = cxx_private_preprocessor_info(
        ctx = ctx,
        headers_layout = cxx_get_regular_cxx_headers_layout(ctx),
        raw_headers = ctx.attrs.raw_headers,
        extra_preprocessors = [],
        non_exported_deps = non_exported_deps,
        is_test = False,
    )

    inherited_non_exported_preprocessor_infos = cxx_inherited_preprocessor_infos(non_exported_deps)

    preprocessor = cxx_merge_cpreprocessors(
        ctx.actions,
        [own_non_exported_preprocessor_info],
        inherited_non_exported_preprocessor_infos,
    )

    headers_tag = ctx.actions.artifact_tag()

    objects = []

    toolchain = get_cxx_toolchain_info(ctx)
    for src in ctx.attrs.srcs:
        rc_output = ctx.actions.declare_output(
            "__objects__",
            "{}.res".format(src.short_path),
            has_content_based_path = False,
        )
        rc_cmd = cmd_args(
            toolchain.rc_compiler_info.compiler,
            toolchain.rc_compiler_info.compiler_flags,
            cmd_args(rc_output.as_output(), format = "/fo{}"),
            headers_tag.tag_artifacts(preprocessor.set.project_as_args("args")),
            headers_tag.tag_artifacts(preprocessor.set.project_as_args("include_dirs")),
            src,
        )

        ctx.actions.run(
            rc_cmd,
            category = "rc_compile",
        )

        cvtres_output = ctx.actions.declare_output(
            "__objects__",
            "{}.obj".format(src.short_path),
            has_content_based_path = False,
        )
        cvtres_cmd = cmd_args(
            toolchain.cvtres_compiler_info.compiler,
            toolchain.cvtres_compiler_info.compiler_flags,
            cmd_args(cvtres_output.as_output(), format = "/OUT:{}"),
            rc_output,
        )

        ctx.actions.run(
            cvtres_cmd,
            category = "cvtres_compile",
        )

        objects.append(cvtres_output)

    link = LinkInfo(
        name = ctx.attrs.name,
        linkables = [
            ObjectsLinkable(
                objects = objects,
                linker_type = toolchain.linker_info.type,
                link_whole = True,
            )
        ],
    )

    providers = [
        DefaultInfo(default_output = None),
        EMPTY_SHARED_LIBRARY_INFO,
        EMPTY_LINK_GROUP_LIB_INFO,
        create_linkable_graph(ctx),
        create_merged_link_info(
            ctx,
            toolchain.pic_behavior,
            {output_style: LinkInfos(default = link) for output_style in LibOutputStyle},
        ),
    ]

    return providers

windows_resource = prelude_rule(
    name = "windows_resource",
    impl = windows_resource_impl,
    docs = """
        A `windows_resource()` rule specifies a set of Window's Resource File (.rc) that
        are compiled into object files.

        The files are compiled into .res files using rc.exe and then compiled into object files
        using cvtres.exe.
        They are not part of cxx_library because Microsoft's linker ignores the resources
        unless they are specified as an object file, meaning including them in a possibly static
        library is unintuitive.
    """,
    examples = """
        ```
        # A rule that includes a single .rc file and compiles it into an object file.
        windows_resource(
          name = "resources",
          srcs = [
            "resources.rc",
          ],
        )

        # A rule that links against the above windows_resource rule.
        cxx_binary(
          name = "app",
          srcs = [
            "main.cpp",
          ],
          deps = [
            ":resources"
          ],
        )
        ```
    """,
    further = None,
    attrs = (
        cxx_common.srcs_arg()
        | cxx_common.headers_arg()
        | cxx_common.header_namespace_arg()
        | cxx_common.raw_headers_arg()
        | cxx_common.include_directories_arg()
        | {
            "deps": attrs.list(attrs.dep(), default = []),
        }
        | buck.labels_arg()
        | buck.contacts_arg()
        | {
            "_cxx_toolchain": toolchains_common.cxx(),
        }
    ),
)
