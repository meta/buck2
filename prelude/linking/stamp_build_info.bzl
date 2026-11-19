# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load(
    "@prelude//cxx:cxx_library_utility.bzl",
    "cxx_is_gnu",
)
load("@prelude//linking:add_elf_sections.bzl", "add_elf_sections")
load(
    "@prelude//linking:link_info.bzl",
    "LinkArgs",  # @unused Used as a type
    "dedupe_dep_metadata",
    "get_link_info",
    "truncate_dep_metadata",
)

PRE_STAMPED_SUFFIX = "-pre_stamped"

def cxx_stamp_build_info(ctx: AnalysisContext, link_groups_enabled: bool = False) -> bool:
    if getattr(ctx.attrs, "_generated_build_info_enabled", False):
        if not getattr(ctx.attrs, "build_info_as_shared_library", True):
            return False
        spec = ctx.attrs._generated_build_info_spec
        return bool((spec.get("link_as_shared_library", False) or link_groups_enabled) and cxx_is_gnu(ctx))

    generated_build_info = getattr(ctx.attrs, "_generated_build_info_spec", {})
    if generated_build_info.get("enabled", False):
        if not getattr(ctx.attrs, "build_info_as_shared_library", True):
            return False
        return bool((generated_build_info.get("link_as_shared_library", False) or link_groups_enabled) and cxx_is_gnu(ctx))

    if not hasattr(ctx.attrs, "_build_info") or not ctx.attrs._build_info or not cxx_is_gnu(ctx):
        return False

    late_build_info_stamping = getattr(ctx.attrs, "_late_build_info_stamping", None)
    return late_build_info_stamping == None or late_build_info_stamping

def _get_library_versions(links: list[LinkArgs] | None) -> str:
    if not links:
        return ""

    metadatas = []
    for args in links:
        if args.tset != None:
            for info in args.tset.infos.traverse():
                metadatas.extend(get_link_info(info).metadata)
        elif args.infos != None:
            for info in args.infos:
                metadatas.extend(info.metadata)

    versions = [metadata.version for metadata in truncate_dep_metadata(dedupe_dep_metadata(metadatas))]
    return ";".join(versions)

def stamp_build_info(
    ctx: AnalysisContext,
    obj: Artifact,
    stamped_output: Artifact | None = None,
    has_content_based_path: bool = False,
    links: list[LinkArgs] | None = None,
    build_info_json: Artifact | None = None,
    link_groups_enabled: bool = False,
) -> Artifact:
    """
    If necessary, add fb_build_info section to binary via late-stamping
    """
    if cxx_stamp_build_info(ctx, link_groups_enabled = link_groups_enabled):
        if build_info_json == None:
            build_info = dict(ctx.attrs._build_info)
            library_versions = _get_library_versions(links)
            if library_versions:
                build_info["library_versions"] = library_versions
            build_info_json = ctx.actions.write_json(obj.short_path + "-build-info.json", build_info, has_content_based_path = has_content_based_path)
        if not stamped_output:
            stem, ext = paths.split_extension(obj.short_path)
            if obj.short_path.endswith(PRE_STAMPED_SUFFIX):
                name = obj.short_path.removesuffix(PRE_STAMPED_SUFFIX)
            elif stem.endswith(PRE_STAMPED_SUFFIX):
                name = stem.removesuffix(PRE_STAMPED_SUFFIX) + ext
            else:
                name = stem + "-stamped" + ext
            stamped_output = ctx.actions.declare_output(name, has_content_based_path = has_content_based_path)

        toolchain = get_cxx_toolchain_info(ctx)

        # elf_stamp adds the section itself when the link did not reserve
        # one, so stamping has no link-side prerequisites. Exec-platform
        # toolchains omit elf_stamp (to break the toolchain -> elf_stamp ->
        # toolchain cycle); objcopy adds the section there instead.
        elf_stamp = toolchain.binary_utilities_info.elf_stamp
        if not elf_stamp:
            return add_elf_sections(
                ctx,
                obj,
                {"fb_build_info": build_info_json},
                stamped_output,
                category = "stamp_build_info",
            )

        cmd = cmd_args([
            elf_stamp,
            "--section",
            cmd_args(build_info_json, format = "fb_build_info={}"),
            obj,
            stamped_output.as_output(),
        ])

        # This can be run remotely, but it's often cheaper to do this locally for large
        # binaries, especially on CI using limited hybrid
        prefer_local = not getattr(ctx.attrs, "optimize_for_action_throughput", False)

        ctx.actions.run(
            cmd,
            identifier = obj.short_path,
            category = "stamp_build_info",
            prefer_local = prefer_local,
            prefer_remote = not prefer_local,
            allow_cache_upload = toolchain.cxx_compiler_info.allow_cache_upload,
        )
        return stamped_output
    return obj
