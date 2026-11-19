# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# BOLT (Binary Optimization Layout Tool) is a post link profile guided optimizer used for
# performance-critical services in fbcode: https://www.internalfb.com/intern/wiki/HHVM-BOLT/

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactTSet",
    "project_artifacts",
)
load(":cxx_context.bzl", "get_cxx_toolchain_info")

PRE_BOLT_SUFFIX = "-wrapper"

CxxBoltOutput = record(
    output = field(Artifact),
    dwo_output = field(Artifact | None),
)

def cxx_use_bolt(ctx: AnalysisContext) -> bool:
    cxx_toolchain_info = get_cxx_toolchain_info(ctx)
    return cxx_toolchain_info.bolt_enabled and ctx.attrs.bolt_profile != None

def bolt(
    ctx: AnalysisContext,
    prebolt_output: Artifact,
    external_debug_info: ArtifactTSet,
    identifier: [str, None],
    generate_dwp: bool,
    allow_cache_upload: bool = False,
) -> CxxBoltOutput:
    output_name = prebolt_output.short_path.removesuffix(PRE_BOLT_SUFFIX)
    postbolt_output = ctx.actions.declare_output(output_name, has_content_based_path = False)
    dwo_output = None
    cxx_toolchain_info = get_cxx_toolchain_info(ctx)
    bolt_tool = cxx_toolchain_info.binary_utilities_info.bolt

    if not bolt_tool or not cxx_use_bolt(ctx):
        fail("Cannot use bolt if bolt tool is not available or bolt profile is not available")

    materialized_external_debug_info = project_artifacts(ctx.actions, external_debug_info)

    # bolt command format:
    # {llvm_bolt} {input_bin} -o $OUT -data={fdata} {args}
    args = cmd_args(
        bolt_tool,
        prebolt_output,
        "-o",
        postbolt_output.as_output(),
        cmd_args(ctx.attrs.bolt_profile, format = "-data={}"),
        ctx.attrs.bolt_flags,
        hidden = materialized_external_debug_info,
    )

    if generate_dwp:
        dwo_output = ctx.actions.declare_output(output_name + ".dwo.d", dir = True, has_content_based_path = False)
        args.add(cmd_args(dwo_output.as_output(), format = "--dwarf-output-path={}"))
        args = cmd_args(
            "/bin/sh",
            "-c",
            cmd_args(dwo_output.as_output(), format = 'mkdir -p {}; "$@"'),
            '""',
            args,
        )

    strip_stapsdt = hasattr(ctx.attrs, "strip_stapsdt") and ctx.attrs.strip_stapsdt
    ctx.actions.run(
        args,
        category = "bolt",
        identifier = identifier,
        local_only = get_cxx_toolchain_info(ctx).linker_info.link_binaries_locally,
        allow_cache_upload = allow_cache_upload and not strip_stapsdt,
    )

    output = postbolt_output

    if strip_stapsdt:
        stripped_postbolt_output = ctx.actions.declare_output(output_name + "-nostapsdt", has_content_based_path = False)
        ctx.actions.run(
            # We --rename-section instead of --remove-section because objcopy's processing
            # in an invalid ELF file
            cmd_args([
                get_cxx_toolchain_info(ctx).binary_utilities_info.objcopy,
                "--rename-section",
                ".stapsdt.base=.deleted_stapsdt_base_section",
                postbolt_output,
                stripped_postbolt_output.as_output(),
            ]),
            category = "bolt_strip_stapsdt",
            identifier = identifier,
            # This can execute in RE, but is cheaper to do locally especially for large binaries that
            # are already locally materialized
            prefer_local = True,
            allow_cache_upload = allow_cache_upload,
        )
        output = stripped_postbolt_output

    return CxxBoltOutput(output = output, dwo_output = dwo_output)
