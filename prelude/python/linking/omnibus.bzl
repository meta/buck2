# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load(
    "@prelude//cxx:omnibus.bzl",
    "create_omnibus_libraries",
    "get_excluded",
    "get_omnibus_graph",
    "get_roots",
)
load("@prelude//linking:link_info.bzl", "LinkedObject")  # @unused Used as a type
load(
    "@prelude//linking:linkable_graph.bzl",
    "create_linkable_graph",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibrary",  # @unused Used as a type
)
load("@prelude//python:python.bzl", "python_attr_preload_deps")
load(
    "@prelude//python:toolchain.bzl",
    "PythonToolchainInfo",  # @unused Used as a type
)

OmnibusMetadataInfo = provider(
    # @unsorted-dict-items
    fields = {"omnibus_libs": provider_field(typing.Any, default = None), "omnibus_graph": provider_field(typing.Any, default = None)},
)

def process_omnibus_linking(
    ctx: AnalysisContext,
    deps: list[Dependency],
    extensions: dict[str, (LinkedObject, Label)],
    python_toolchain: PythonToolchainInfo,
    extra: dict[str, typing.Any],
) -> (
    list[(SharedLibrary, str)],
    dict[str, (LinkedObject, Label)],
):
    # If we're using omnibus linking, re-link libraries and extensions and
    # update the libraries we'll pull into the final binary.

    # Collect omnibus info from deps.
    linkable_graph = create_linkable_graph(
        ctx,
        deps = deps,
    )

    omnibus_graph = get_omnibus_graph(
        graph = linkable_graph,
        # Add in any potential native root targets from our first-order deps.
        roots = get_roots(deps),
        # Exclude preloaded deps from omnibus linking, to prevent preloading
        # the monolithic omnibus library.
        excluded = get_excluded(deps = python_attr_preload_deps(ctx)),
    )

    # Link omnibus libraries.
    omnibus_libs = create_omnibus_libraries(
        ctx,
        omnibus_graph,
        extra_ldflags = (
            # TODO(agallagher): Should these "binary" linker flags comes
            # from the Python toolchain instead?
            get_cxx_toolchain_info(ctx).linker_info.binary_linker_flags + python_toolchain.linker_flags + ctx.attrs.linker_flags
        ),
        prefer_stripped_objects = ctx.attrs.prefer_stripped_native_objects,
        enable_distributed_thinlto = ctx.attrs.enable_distributed_thinlto,
        anonymous = ctx.attrs.deduplicate_merged_link_roots,
    )

    # Extract re-linked extensions.
    extensions = {dest: (omnibus_libs.roots[label].shared_library, label) for dest, (_, label) in extensions.items()}
    shared_libs = [(shlib, "") for shlib in omnibus_libs.libraries]

    omnibus_providers = []

    if omnibus_libs.omnibus != None:
        omnibus_link_result = omnibus_libs.omnibus
        omnibus_linked_obj = omnibus_link_result.linked_object

        sub_targets = {}
        sub_targets["dwp"] = [DefaultInfo(default_output = omnibus_linked_obj.dwp if omnibus_linked_obj.dwp else None)]
        if omnibus_link_result.linker_map_data != None:
            sub_targets["linker-map"] = [
                DefaultInfo(default_output = omnibus_link_result.linker_map_data.map, other_outputs = [omnibus_link_result.linker_map_data.binary])
            ]
        omnibus_info = DefaultInfo(
            default_output = omnibus_linked_obj.output,
            sub_targets = sub_targets,
        )
    else:
        omnibus_info = DefaultInfo()
    omnibus_providers.append(omnibus_info)

    if python_toolchain.emit_omnibus_metadata:
        omnibus_providers.append(
            OmnibusMetadataInfo(
                omnibus_libs = omnibus_libs,
                omnibus_graph = omnibus_graph,
            ),
        )

        exclusion_roots = ctx.actions.write_json("omnibus/exclusion_roots.json", omnibus_libs.exclusion_roots, has_content_based_path = False)
        extra["omnibus-exclusion-roots"] = [DefaultInfo(default_output = exclusion_roots)]

        roots = ctx.actions.write_json("omnibus/roots.json", omnibus_libs.roots, has_content_based_path = False)
        extra["omnibus-roots"] = [DefaultInfo(default_output = roots)]

        omnibus_excluded = ctx.actions.write_json("omnibus/excluded.json", omnibus_libs.excluded, has_content_based_path = False)
        extra["omnibus-excluded"] = [DefaultInfo(default_output = omnibus_excluded)]

        omnibus_graph_json = ctx.actions.write_json("omnibus_graph.json", omnibus_graph, has_content_based_path = False)
        extra["linkable-graph"] = [DefaultInfo(default_output = omnibus_graph_json)]

    extra["omnibus"] = omnibus_providers
    return shared_libs, extensions
