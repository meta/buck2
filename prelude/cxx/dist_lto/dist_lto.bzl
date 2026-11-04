# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "make_artifact_tset",
    "project_artifacts",
)
load("@prelude//:paths.bzl", "paths")
load(
    "@prelude//cxx:cxx_bolt.bzl",
    "PRE_BOLT_SUFFIX",
    "bolt",
    "cxx_use_bolt",
)
load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load(
    "@prelude//cxx:cxx_link_utility.bzl",
    "cxx_link_cmd_parts",
    "gc_sections_args",
    "linker_map_args",
)
load("@prelude//cxx:cxx_toolchain_types.bzl", "LinkerType")
load("@prelude//cxx:debug.bzl", "SplitDebugMode")
load(
    "@prelude//cxx:dwp.bzl",
    "run_dwp_action",
)
load("@prelude//cxx:link_types.bzl", "LinkOptions", "get_dwp_execution_preference")
load("@prelude//linking:execution_preference.bzl", "get_action_execution_attributes")
load(
    "@prelude//linking:link_info.bzl",
    "ArchiveLinkable",
    "FrameworksLinkable",
    "LinkInfo",
    "LinkedObject",
    "ObjectsLinkable",
    "SharedLibLinkable",  # @unused Used as a type
    "SwiftmoduleLinkable",
    "append_linkable_args",
    "map_to_link_infos",
    "serialize_linkable_for_thinlto",
    "unpack_external_debug_info",
    "unpack_link_args",
)
load("@prelude//linking:stamp_build_info.bzl", "cxx_stamp_build_info", "stamp_build_info")
load("@prelude//linking:strip.bzl", "strip_object")
load("@prelude//utils:lazy.bzl", "lazy")

# `_BitcodeLinkData` holds data
# that needs to be tracked across ThinLTO phases for Bitcode (LLVM IR) files.
# Bitcode is produced from Phase 1 (compile).
# Tracking data is held to orchestrate between
# Phase 2 (Thin Link) and Phase 3 (ThinLTO Backends).
# Phase 2 (Thin Link) writes a plan for Phase 3 (ThinLTO Backends).
# The plan contains which other bitcode files are needed to perform
# Phase 3.
_BitcodeLinkData = record(
    # `id` is the number that will be used to refer back to this record across the ThinLTO phases.
    id = int,
    name = str,
    initial_object = Artifact,
    bc_file = Artifact,
    plan = Artifact,
    opt_object = Artifact,
    external_debug_info = Artifact | None,
)

# `_ArchiveLinkData` holds data
# that needs to be tracked across ThinLTO phases for Archive (.a) files that contain Bitcode/LLVM IR files.
# See `_BitcodeLinkData` for how this data is used across the Phases.
# `_ArchiveLinkData` is unarchived and tracking for that is held here.
_ArchiveLinkData = record(
    # `id` is the number that will be used to refer back to this record across the ThinLTO phases.
    id = int,
    name = str,
    manifest = Artifact,
    # A file containing paths to artifacts that are known to reside in opt_objects_dir.
    opt_manifest = Artifact,
    objects_dir = Artifact,
    opt_objects_dir = Artifact,
    indexes_dir = Artifact,
    plan = Artifact,
    link_whole = bool,
    prepend = bool,
    # Directory for .dwo files when split_debug_mode is "split".
    dwo_dir = Artifact | None,
)

_DataType = enum(
    "bitcode",
    "archive",
    "cmd_args",
)

# `_LinkDataVariant` is a discriminated union
# that holds the arguments that is feed into the linker and
# tracked throughout the ThinLTO phases.
_LinkDataVariant = record(
    # discriminant/tag field to determine what type of LinkData this is.
    data_type = _DataType,
    # The link_data field has the following invariant:
    # - `list[_BitcodeLinkData]` when data_type is `_DataType("bitcode")`
    # - `_ArchiveLinkData` when data_type is `_DataType("archive")`
    # - `None` when data_type is `_DataType("cmd_args")`
    link_data = list[_BitcodeLinkData] | _ArchiveLinkData | None,
    # The linkable that this is derived from.
    # This field is used to get the `cmd_args` form.
    # FrameworksLinkable / SwiftmoduleLinkable are skipped before a variant is
    # built, so they never appear here.
    origin = ArchiveLinkable | SharedLibLinkable | ObjectsLinkable,
)

# `_ThinLinkInfo` groups together pre/post flags and a list of LinkData.
_ThinLinkInfo = record(
    pre_flags = list,
    post_flags = list,
    link_data_variants = list[_LinkDataVariant],
)

def cxx_gnu_dist_link(
    ctx: AnalysisContext,
    # The destination for the link output.
    output: Artifact,
    opts: LinkOptions,
    linker_map: Artifact | None = None,
    gc_sections_output: Artifact | None = None,
    # This action will only happen if split_dwarf is enabled via the toolchain.
    dwp_tool_available: bool = True,
    executable_link: bool = True,
    build_info_json: Artifact | None = None,
) -> LinkedObject:
    """
    Perform a distributed thin-lto link into the supplied output

    Distributed thinlto splits the link into three stages:
    1. global "indexing" step
    2. many individual compilation unit optimization steps
    3. final global link step

    The 2nd and 3rd of those are done just by constructing compiler/linker commands (in dynamic_output
    sections) using the output of the first.

    For the first, we need to post-process the linker index output to get it into a form
    that is easy for us to consume from within bzl.
    """

    links = opts.links
    binary_links = opts.binary_links

    # A category suffix that will be added to the category of the link action that is generated.
    category_suffix = opts.category_suffix

    # An identifier that will uniquely name this link action in the context of a category. Useful for
    # differentiating multiple link actions in the same rule.
    identifier = opts.identifier

    # Names derived from the identifier below become artifact paths, and callers
    # pass identifiers containing spaces and brackets.
    normalized_identifier = identifier.replace("[", "_").replace("]", "_").replace(" ", "_") if identifier != None else None

    enable_late_build_info_stamping = executable_link and cxx_stamp_build_info(ctx)

    # Unlike the plain link, a `None` (no preference) narrows to `False` here,
    # because this also feeds `bolt` and `strip_object`, which take a plain bool.
    enable_cache_upload = opts.allow_cache_upload or enable_late_build_info_stamping

    link_action_execution_properties = get_action_execution_attributes(opts.link_execution_preference)
    dwp_action_execution_properties = get_action_execution_attributes(get_dwp_execution_preference(opts))
    enable_bolt = executable_link and cxx_use_bolt(ctx)

    def make_cat(c: str) -> str:
        """Used to make sure categories for our actions include the provided suffix"""
        if category_suffix != None:
            return c + "_" + category_suffix
        return c

    def make_id(i: str) -> str:
        """Used to make sure identifiers for our actions include the provided identifier"""
        if normalized_identifier != None:
            return normalized_identifier + "_" + i
        return i

    recorded_outputs = {}

    def name_for_obj(link_name: str, object_artifact: Artifact) -> str:
        """Creates a unique name/path we can use for a particular object file input"""
        prefix = "{}/{}".format(link_name, object_artifact.short_path)

        # it's possible (though unlikely) that we can get duplicate name/short_path, so just uniquify them
        if prefix in recorded_outputs:
            recorded_outputs[prefix] += 1
            extra = recorded_outputs[prefix]
            prefix = "{}-{}".format(prefix, extra)
        else:
            recorded_outputs[prefix] = 1
        return prefix

    names = {}

    def name_for_link(info: LinkInfo) -> str:
        """Creates a unique name for a LinkInfo that we are consuming"""
        name = info.name or "unknown"
        if name not in names:
            names[name] = 1
        else:
            names[name] += 1
            name += "-{}".format(names[name])
        return make_id(name)

    link_infos = map_to_link_infos(links)

    cxx_toolchain = get_cxx_toolchain_info(ctx)
    lto_planner = cxx_toolchain.internal_tools.dist_lto.planner[LinkerType("gnu")]
    lto_opt = cxx_toolchain.internal_tools.dist_lto.opt[LinkerType("gnu")]
    lto_prepare = cxx_toolchain.internal_tools.dist_lto.prepare[LinkerType("gnu")]
    lto_copy = cxx_toolchain.internal_tools.dist_lto.copy
    split_debug_mode = cxx_toolchain.split_debug_mode

    PREPEND_ARCHIVE_NAMES = [
        # T130644072: If linked with `--whole-archive`, Clang builtins must be at the
        # front of the argument list to avoid conflicts with identically-named Rust
        # symbols from the `compiler_builtins` crate.
        #
        # Once linking of C++ binaries starts to use `rlib`s, this may no longer be
        # necessary, because our Rust `rlib`s won't need to contain copies of
        # `compiler_builtins` to begin with, unlike our Rust `.a`s which presently do
        # (T130789782).
        "clang_rt.builtins",
    ]

    # `link_data_repository` is a bridge between the outputs of `dynamic_plan` (Phase 2 Thin Link)
    # and the inputs into `dynamic_optimize` and `dynamic_optimize_archive` (Phase 3 ThinLTO Backends).
    # Note: The index into `link_data_repository` is important as it's how things will appear in
    # the plans produced by indexing. That allows us to map those indexes back to
    # the actual artifacts.
    link_data_repository = []  # type: list[_BitcodeLinkData | _ArchiveLinkData]

    # thin_link_infos contains __ALL__ the arguments used in the Phase 2 Thin link step.
    # It contains LLVM IR bitcode files, machine object files, shared objects, linker flags, etc.
    thin_link_infos = []  # type: list[_ThinLinkInfo]

    # Information used to construct the dynamic plan:
    plan_inputs = []  # type: list[Artifact]
    plan_outputs = []  # type: list[OutputArtifact]

    # Information used to construct the opt dynamic outputs:
    archive_opt_manifests = []  # type: list[Artifact]

    prepare_cat = make_cat("thin_lto_prepare")

    # The `for` loop below reads each `LinkInfo` and transforms it into a `_ThinLinkInfo`.
    # `_ThinLinkInfo` will carry the additional structures used through out the multiple phases of ThinLTO.
    for link in link_infos:
        link_name = name_for_link(link)

        link_data_variants = []  # type: list[_LinkDataVariant]
        for linkable in link.linkables:
            if isinstance(linkable, ObjectsLinkable):
                data_type = _DataType("bitcode")
                link_data = []  # type: list[_BitcodeLinkData]

                for obj in linkable.objects:
                    name = name_for_obj(link_name, obj)
                    bc_output = ctx.actions.declare_output(name + ".thinlto.bc", has_content_based_path = False)
                    plan_output = ctx.actions.declare_output(name + ".opt.plan", has_content_based_path = False)
                    opt_output = ctx.actions.declare_output(name + ".opt.o", has_content_based_path = False)
                    external_debug_info = None
                    if split_debug_mode == SplitDebugMode("split"):
                        external_debug_info = ctx.actions.declare_output(name + ".opt.dwo", has_content_based_path = False)
                    elif split_debug_mode == SplitDebugMode("single"):
                        external_debug_info = opt_output

                    bitcode_link_data = _BitcodeLinkData(
                        id = len(link_data_repository),
                        name = name,
                        initial_object = obj,
                        bc_file = bc_output,
                        plan = plan_output,
                        opt_object = opt_output,
                        external_debug_info = external_debug_info,
                    )
                    link_data_repository.append(bitcode_link_data)
                    link_data.append(bitcode_link_data)
                    plan_outputs.extend([bc_output.as_output(), plan_output.as_output()])
            elif isinstance(linkable, ArchiveLinkable) and linkable.supports_lto:
                data_type = _DataType("archive")
                # Our implementation of Distributed ThinLTO operates on individual objects, not archives. Since these
                # archives might still contain LTO-able bitcode, we first extract the objects within the archive into
                # another directory and write a "manifest" containing the list of objects that the archive contained.
                #
                # Later actions in the LTO compilation pipeline will read this manifest and dynamically dispatch
                # actions on the objects that the manifest reports.

                name = name_for_obj(link_name, linkable.archive.artifact)
                archive_manifest = ctx.actions.declare_output("%s/%s/manifest.json" % (prepare_cat, name), has_content_based_path = False)
                archive_objects = ctx.actions.declare_output("%s/%s/objects" % (prepare_cat, name), dir = True, has_content_based_path = False)
                archive_opt_objects = ctx.actions.declare_output("%s/%s/opt_objects" % (prepare_cat, name), dir = True, has_content_based_path = False)
                archive_indexes = ctx.actions.declare_output("%s/%s/indexes" % (prepare_cat, name), dir = True, has_content_based_path = False)
                archive_plan = ctx.actions.declare_output("%s/%s/plan.json" % (prepare_cat, name), has_content_based_path = False)
                archive_opt_manifest = ctx.actions.declare_output("%s/%s/opt_objects.manifest" % (prepare_cat, name), has_content_based_path = False)
                archive_dwo_dir = None
                if split_debug_mode == SplitDebugMode("split"):
                    archive_dwo_dir = ctx.actions.declare_output("%s/%s/dwo" % (prepare_cat, name), dir = True, has_content_based_path = False)
                prepare_args = cmd_args([
                    lto_prepare,
                    "--manifest-out",
                    archive_manifest.as_output(),
                    "--objects-out",
                    archive_objects.as_output(),
                    "--ar",
                    cxx_toolchain.linker_info.archiver,
                    "--archive",
                    linkable.archive.artifact,
                    "--name",
                    name,
                ])
                ctx.actions.run(prepare_args, category = make_cat("thin_lto_prepare"), identifier = name)

                link_data = _ArchiveLinkData(
                    id = len(link_data_repository),
                    name = name,
                    manifest = archive_manifest,
                    opt_manifest = archive_opt_manifest,
                    objects_dir = archive_objects,
                    opt_objects_dir = archive_opt_objects,
                    indexes_dir = archive_indexes,
                    plan = archive_plan,
                    link_whole = linkable.link_whole,
                    prepend = link_name in PREPEND_ARCHIVE_NAMES,
                    dwo_dir = archive_dwo_dir,
                )
                link_data_repository.append(link_data)
                archive_opt_manifests.append(archive_opt_manifest)
                plan_inputs.append(archive_manifest)
                plan_outputs.extend([archive_indexes.as_output(), archive_plan.as_output()])
            elif isinstance(linkable, FrameworksLinkable) or isinstance(linkable, SwiftmoduleLinkable):
                # These add no linker inputs on GNU: append_linkable_args no-ops them
                # and serialize_linkable_for_thinlto rejects them, so there is nothing
                # to emit here (the Darwin planner skips them likewise). Use `continue`,
                # not `pass`: link_data_variants is appended below the if/elif/else, so
                # falling through would emit a variant from the previous linkable.
                continue
            else:
                data_type = _DataType("cmd_args")
                link_data = None

            link_data_variants.append(
                _LinkDataVariant(
                    data_type = data_type,
                    link_data = link_data,
                    origin = linkable,
                )
            )

        thin_link_infos.append(
            _ThinLinkInfo(
                pre_flags = link.pre_flags,
                post_flags = link.post_flags,
                link_data_variants = link_data_variants,
            )
        )

    index_argsfile_out = ctx.actions.declare_output(output.short_path + ".thinlto_index_argsfile", has_content_based_path = False)
    final_link_index = ctx.actions.declare_output(output.short_path + ".final_link_index", has_content_based_path = False)

    def dynamic_plan(
        link_plan: Artifact,
        index_argsfile_out: Artifact,
        final_link_index: Artifact,
    ) -> None:
        def plan(ctx: AnalysisContext, artifacts, outputs):
            # index link command args
            prepend_index_args = cmd_args()
            index_args = cmd_args()

            # index_meta is the comprehensive source-of-truth for Phase 2 Thin Link.
            # It holds the record of all the flags, linkable files, and their associated data.
            # See comments in dist_lto_planner.py for semantics on the values that are pushed into index_meta.
            index_meta = []

            # buildifier: disable=uninitialized
            for thin_link_info in thin_link_infos:
                metadata_entry = {
                    "linkables": [],
                    "post_flags": thin_link_info.post_flags,
                    "pre_flags": thin_link_info.pre_flags,
                }
                index_meta.append(metadata_entry)

                if thin_link_info.pre_flags:
                    index_args.add(thin_link_info.pre_flags)

                for link_data_variant in thin_link_info.link_data_variants:
                    if link_data_variant.data_type == _DataType("bitcode"):
                        append_linkable_args(index_args, link_data_variant.origin)

                        for link_data in link_data_variant.link_data:
                            metadata_linkable = {
                                "idx": link_data.id,
                                "output": outputs[link_data.bc_file].as_output(),
                                "path": link_data.initial_object,
                                "plan_output": outputs[link_data.plan].as_output(),
                                "type": "bitcode",
                            }
                            metadata_entry["linkables"].append(metadata_linkable)

                    elif link_data_variant.data_type == _DataType("archive"):
                        link_data = link_data_variant.link_data
                        manifest = artifacts[link_data.manifest].read_json()

                        metadata_linkable = {
                            "archive_idx": link_data.id,
                            "archive_index_dir": outputs[link_data.indexes_dir].as_output(),
                            "archive_name": link_data.name,
                            # `opt_objects_dir` is produced downstream of this plan, so it
                            # can only be passed as a path: `ignore_artifacts` keeps it out
                            # of the meta file's inputs, and `delimiter` makes it serialize
                            # as a string rather than a single-element list.
                            "archive_opt_objects_dir": cmd_args(link_data.opt_objects_dir, delimiter = "", ignore_artifacts = True),
                            "archive_plan": outputs[link_data.plan].as_output(),
                            "link_whole": link_data.link_whole,
                            "objects": [] if manifest["objects"] else None,
                            "prepend": link_data.prepend,
                            "type": "archive",
                        }
                        metadata_entry["linkables"].append(metadata_linkable)

                        if not manifest["objects"]:
                            # Despite not having any objects (and thus not needing a plan), we still need to bind the plan output.
                            ctx.actions.write(outputs[link_data.plan].as_output(), "{}")
                            cmd = cmd_args(["/bin/sh", "-c", "mkdir", "-p", outputs[link_data.indexes_dir].as_output()])
                            ctx.actions.run(cmd, category = make_cat("thin_lto_mkdir"), identifier = link_data.name)
                            continue

                        archive_args = prepend_index_args if link_data.prepend else index_args

                        archive_args.add(cmd_args(hidden = link_data.objects_dir))

                        if not link_data.link_whole:
                            archive_args.add("-Wl,--start-lib")

                        for obj in manifest["objects"]:
                            metadata_linkable["objects"].append({
                                "idx": link_data.id,
                                "output": "",
                                "path": obj,
                                "plan_output": "",
                            })
                            archive_args.add(obj)

                        if not link_data.link_whole:
                            archive_args.add("-Wl,--end-lib")
                    elif link_data_variant.data_type == _DataType("cmd_args"):
                        linkable_args = cmd_args()
                        append_linkable_args(linkable_args, link_data_variant.origin)
                        index_args.add(linkable_args)

                        metadata_linkable = {
                            "cmd_args": linkable_args,
                            "object": serialize_linkable_for_thinlto(link_data_variant.origin),
                            "type": "cmd_args",
                        }
                        metadata_entry["linkables"].append(metadata_linkable)
                    else:
                        fail("Unknown data type:", link_data_variant)

                if thin_link_info.post_flags:
                    index_args.add(thin_link_info.post_flags)

            prepend_index_args.add(index_args)
            index_argfile, _ = ctx.actions.write(
                outputs[index_argsfile_out].as_output(),
                prepend_index_args,
                allow_args = True,
            )

            index_cat = make_cat("thin_lto_index")
            index_file_out = ctx.actions.declare_output(make_id(index_cat) + "/index", has_content_based_path = False)
            index_out_dir = cmd_args(index_file_out.as_output(), parent = 1)

            index_cmd_parts = cxx_link_cmd_parts(cxx_toolchain, executable_link)

            index_cmd = index_cmd_parts.link_cmd
            index_cmd.add(cmd_args(index_argfile, format = "@{}", hidden = prepend_index_args))

            # Binary-only link flags (e.g. --wrap) must be present at the index
            # step so symbol resolution matches the final link. They are
            # intentionally omitted from the per-object opt actions so those
            # actions stay identical across links and can be deduplicated.
            for link in binary_links:
                index_cmd.add(unpack_link_args(link))

            # Indexing must not resolve the final binary's content-based path:
            # producing that binary depends on the index being generated here.
            index_cmd.add("-o", cmd_args(index_out_dir, format = "{}/indexing-output"))
            index_cmd.add(cmd_args(index_file_out.as_output(), format = "-Wl,--thinlto-index-only={}"))
            index_cmd.add("-Wl,--thinlto-emit-imports-files")
            index_cmd.add("-Wl,--thinlto-full-index")
            index_cmd.add(cmd_args(index_out_dir, format = "-Wl,--thinlto-prefix-replace=;{}/"))
            index_cmd.add(index_cmd_parts.post_linker_flags)

            index_meta_file = ctx.actions.write_json(
                output.short_path + ".thinlto.meta.json",
                index_meta,
                with_inputs = True,
                has_content_based_path = False,
            )

            plan_cmd = cmd_args(
                [
                    lto_planner,
                    "--meta",
                    index_meta_file,
                    "--index",
                    index_out_dir,
                    "--link-plan",
                    outputs[link_plan].as_output(),
                    "--final-link-index",
                    outputs[final_link_index].as_output(),
                    "--",
                ],
            )
            plan_cmd.add(index_cmd)

            ctx.actions.run(
                plan_cmd,
                category = index_cat,
                identifier = identifier,
                prefer_local = link_action_execution_properties.prefer_local,
                prefer_remote = link_action_execution_properties.prefer_remote,
                local_only = link_action_execution_properties.local_only,
                weight = opts.link_weight,
                force_full_hybrid_if_capable = link_action_execution_properties.full_hybrid,
            )

        # TODO(T117513091) - dynamic_output does not allow for an empty list of dynamic inputs. If we have no archives
        # to process, we will have no dynamic inputs, and the plan action can be non-dynamic.
        #
        # However, buck2 disallows `dynamic_output` with a empty input list. We also can't call our `plan` function
        # directly, since it uses `ctx.outputs` to bind its outputs. Instead of doing Starlark hacks to work around
        # the lack of `ctx.outputs`, we declare an empty file as a dynamic input.
        plan_inputs.append(ctx.actions.write(output.short_path + ".plan_hack.txt", "", has_content_based_path = False))
        plan_outputs.extend([
            link_plan.as_output(),
            index_argsfile_out.as_output(),
            final_link_index.as_output(),
        ])
        ctx.actions.dynamic_output(dynamic = plan_inputs, inputs = [], outputs = plan_outputs, f = plan)

    link_plan_out = ctx.actions.declare_output(output.short_path + ".link-plan.json", has_content_based_path = False)
    dynamic_plan(
        link_plan = link_plan_out,
        index_argsfile_out = index_argsfile_out,
        final_link_index = final_link_index,
    )

    def prepare_opt_flags(link_infos: list[LinkInfo]) -> cmd_args:
        opt_cmd_parts = cxx_link_cmd_parts(cxx_toolchain, executable_link)
        opt_args = opt_cmd_parts.link_cmd

        if cxx_toolchain.linker_info.dist_thin_lto_codegen_flags != None:
            opt_args.add(cxx_toolchain.linker_info.dist_thin_lto_codegen_flags)

        # buildifier: disable=uninitialized
        for link in link_infos:
            for raw_flag in link.pre_flags + link.post_flags:
                opt_args.add(raw_flag)
            opt_args.add(link.dist_thin_lto_codegen_flags)

        opt_args.add(opt_cmd_parts.post_linker_flags)
        return opt_args

    opt_common_flags = prepare_opt_flags(link_infos)

    # Create an argsfile and dump all the flags to be processed later by lto_opt.
    # These flags are common to all opt actions, we don't need an argfile for each action, one
    # for the entire link unit will do.
    opt_argsfile = ctx.actions.declare_output(output.short_path + ".lto_opt_argsfile", has_content_based_path = False)
    ctx.actions.write(opt_argsfile.as_output(), opt_common_flags, allow_args = True)

    # We declare a separate dynamic_output for every object file. It would
    # maybe be simpler to have a single dynamic_output that produced all the
    # opt actions, but an action needs to re-run whenever the analysis that
    # produced it re-runs. And so, with a single dynamic_output, we'd need to
    # re-run all actions when any of the plans changed.
    def dynamic_optimize(name: str, initial_object: Artifact, bc_file: Artifact, plan: Artifact, opt_object: Artifact, external_debug_info: Artifact | None):
        def optimize_object(ctx: AnalysisContext, artifacts, outputs):
            plan_json = artifacts[plan].read_json()

            # A machine-code object has no backend; neither has a bitcode module the
            # thin link did not select. An empty file binds the outputs of either;
            # nothing reads it, because the final link names neither.
            if not plan_json["is_bc"] or plan_json["skip_backend"]:
                ctx.actions.write(outputs[opt_object], "")
                if external_debug_info != None and external_debug_info != opt_object:
                    ctx.actions.write(outputs[external_debug_info], "")
                return

            opt_cmd = cmd_args(lto_opt)
            opt_cmd.add("--out", outputs[opt_object].as_output())
            opt_cmd.add("--input", initial_object)
            opt_cmd.add("--index", bc_file)

            # When invoking opt and llc via clang, clang will not respect IR metadata to generate
            # dwo files unless -gsplit-dwarf is explicitly passed in. In other words, even if
            # 'splitDebugFilename' set in IR 'DICompileUnit', we still need to force clang to tell
            # llc to generate dwo sections.
            #
            # Local thinlto generates .dwo files by default. For distributed thinlto, however, we
            # want to keep all dwo debug info in the object file to reduce the number of files to
            # materialize.
            if split_debug_mode == SplitDebugMode("none"):
                opt_cmd.add("--split-dwarf=none")
            elif split_debug_mode == SplitDebugMode("single"):
                opt_cmd.add("--split-dwarf=single")
            elif split_debug_mode == SplitDebugMode("split"):
                opt_cmd.add("--split-dwarf=split")
                opt_cmd.add("--dwo", outputs[external_debug_info].as_output())

            opt_cmd.add(cmd_args(hidden = opt_common_flags))
            opt_cmd.add("--args", opt_argsfile)

            opt_cmd.add("--")
            opt_cmd.add(cxx_toolchain.cxx_compiler_info.compiler)

            imports = [link_data_repository[idx].initial_object for idx in plan_json["imports"]]
            archives = [link_data_repository[idx].objects_dir for idx in plan_json["archive_imports"]]
            opt_cmd.add(cmd_args(hidden = imports + archives))
            ctx.actions.run(opt_cmd, category = make_cat("thin_lto_opt_object"), identifier = name)

        outputs = [opt_object.as_output()]
        if external_debug_info != None:
            outputs.append(external_debug_info.as_output())
        ctx.actions.dynamic_output(dynamic = [plan], inputs = [], outputs = outputs, f = optimize_object)

    def dynamic_optimize_archive(archive: _ArchiveLinkData):
        def optimize_archive(ctx: AnalysisContext, artifacts, outputs):
            plan_json = artifacts[archive.plan].read_json()
            if "objects" not in plan_json or not plan_json["objects"] or lazy.is_all(lambda e: not e["is_bc"], plan_json["objects"]):
                # Nothing in this directory was lto-able; let's just copy the archive.
                # `opt_objects_dir` is the only dir of an lto-able archive that the final
                # link materializes, and it must hold every member at the member's path
                # relative to the archive's base dir -- the layout the per-member branch
                # below builds, and the one the planner derives final-link paths from.
                ctx.actions.copied_dir(outputs[archive.opt_objects_dir], {"objects": archive.objects_dir})
                ctx.actions.write(outputs[archive.opt_manifest], "")
                if archive.dwo_dir != None:
                    ctx.actions.symlinked_dir(outputs[archive.dwo_dir], {})
                return

            output_dir = {}
            output_manifest = cmd_args()
            dwo_dir = {}
            for entry in plan_json["objects"]:
                # The thin link did not select this member (the planner rejects a
                # skipped member of a whole archive): no backend, and the final
                # link index never names it under `opt_objects_dir`.
                if entry["skip_backend"]:
                    continue

                base_dir = plan_json["base_dir"]
                source_path = paths.relativize(entry["path"], base_dir)
                if not entry["is_bc"]:
                    opt_object = ctx.actions.declare_output("%s/%s" % (make_cat("thin_lto_opt_copy"), source_path), has_content_based_path = False)
                    output_manifest.add(opt_object)
                    copy_cmd = cmd_args(
                        [
                            lto_copy,
                            "--to",
                            opt_object.as_output(),
                            "--from",
                            entry["path"],
                        ],
                        hidden = archive.objects_dir,
                    )
                    ctx.actions.run(copy_cmd, category = make_cat("thin_lto_opt_copy"), identifier = source_path)
                    output_dir[source_path] = opt_object
                    continue

                opt_object = ctx.actions.declare_output("%s/%s" % (make_cat("thin_lto_opt_archive"), source_path), has_content_based_path = False)
                output_manifest.add(opt_object)
                output_dir[source_path] = opt_object

                opt_cmd = cmd_args(lto_opt)
                opt_cmd.add("--out", opt_object.as_output())
                opt_cmd.add("--input", entry["path"])
                opt_cmd.add("--index", entry["bitcode_file"])

                if split_debug_mode == SplitDebugMode("none"):
                    opt_cmd.add("--split-dwarf=none")
                elif split_debug_mode == SplitDebugMode("single"):
                    opt_cmd.add("--split-dwarf=single")
                elif split_debug_mode == SplitDebugMode("split"):
                    opt_cmd.add("--split-dwarf=split")
                    dwo_path = source_path.removesuffix(".o") + ".dwo"
                    dwo_output = ctx.actions.declare_output("%s/%s" % (make_cat("thin_lto_opt_archive"), dwo_path), has_content_based_path = False)
                    dwo_dir[dwo_path] = dwo_output
                    opt_cmd.add("--dwo", dwo_output.as_output())

                opt_cmd.add(cmd_args(hidden = opt_common_flags))
                opt_cmd.add("--args", opt_argsfile)

                opt_cmd.add("--")
                opt_cmd.add(cxx_toolchain.cxx_compiler_info.compiler)

                imports = [link_data_repository[idx].initial_object for idx in entry["imports"]]
                archives = [link_data_repository[idx].objects_dir for idx in entry["archive_imports"]]
                opt_cmd.add(
                    cmd_args(
                        hidden = imports + archives + [archive.indexes_dir, archive.objects_dir],
                    )
                )
                ctx.actions.run(opt_cmd, category = make_cat("thin_lto_opt_archive"), identifier = source_path)

            ctx.actions.symlinked_dir(outputs[archive.opt_objects_dir], output_dir)
            ctx.actions.write(outputs[archive.opt_manifest], output_manifest, allow_args = True)
            if archive.dwo_dir != None:
                ctx.actions.symlinked_dir(outputs[archive.dwo_dir], dwo_dir)

        archive_opt_inputs = [archive.plan]
        archive_opt_outputs = [archive.opt_objects_dir.as_output(), archive.opt_manifest.as_output()]
        if archive.dwo_dir != None:
            archive_opt_outputs.append(archive.dwo_dir.as_output())
        ctx.actions.dynamic_output(dynamic = archive_opt_inputs, inputs = [], outputs = archive_opt_outputs, f = optimize_archive)

    objects_external_debug_info = []
    for link_data in link_data_repository:
        if isinstance(link_data, _BitcodeLinkData):
            external_debug_info = link_data.external_debug_info
            dynamic_optimize(
                name = link_data.name,
                initial_object = link_data.initial_object,
                bc_file = link_data.bc_file,
                plan = link_data.plan,
                opt_object = link_data.opt_object,
                external_debug_info = link_data.external_debug_info,
            )

            if external_debug_info != None:
                objects_external_debug_info.append(external_debug_info)
        elif isinstance(link_data, _ArchiveLinkData):
            dynamic_optimize_archive(link_data)

            # For split mode, add the dwo directory to external_debug_info.
            # For single mode, the debug info is embedded in the opt_objects.
            if split_debug_mode == SplitDebugMode("split") and link_data.dwo_dir != None:
                objects_external_debug_info.append(link_data.dwo_dir)
            elif split_debug_mode == SplitDebugMode("single"):
                objects_external_debug_info.append(link_data.opt_objects_dir)

    def thin_lto_final_link(ctx: AnalysisContext, artifacts, outputs):
        plan = artifacts[link_plan_out].read_json()
        plan_index = {int(k): v for k, v in plan["index"].items()}

        # non_lto_objects are the ones that weren't compiled with thinlto
        # flags. In that case, we need to link against the original object.
        non_lto_objects = {int(k): 1 for k in plan["non_lto_objects"]}
        current_index = 0
        link_cmd_hidden = cmd_args()
        for link in link_infos:
            for linkable in link.linkables:
                if isinstance(linkable, ObjectsLinkable):
                    for obj in linkable.objects:
                        if current_index in plan_index:
                            link_cmd_hidden.add(link_data_repository[current_index].opt_object)
                        elif current_index in non_lto_objects:
                            link_cmd_hidden.add(obj)
                        current_index += 1
                elif isinstance(linkable, ArchiveLinkable) and linkable.supports_lto:
                    current_index += 1
                else:
                    append_linkable_args(link_cmd_hidden, linkable)

            # pre/post flags may reference artifacts (e.g. linker scripts via
            # `-Wl,--script=path`).
            link_cmd_hidden.add(link.pre_flags)
            link_cmd_hidden.add(link.post_flags)

        link_cmd_parts = cxx_link_cmd_parts(cxx_toolchain, executable_link)
        link_cmd = link_cmd_parts.link_cmd

        # buildifier: disable=uninitialized
        for link_data in link_data_repository:
            if isinstance(link_data, _ArchiveLinkData):
                link_cmd_hidden.add(link_data.opt_objects_dir)

        link_cmd.add(cmd_args(final_link_index, format = "@{}", hidden = link_cmd_hidden))
        for link in binary_links:
            link_cmd.add(unpack_link_args(link))
        link_cmd.add("-o", outputs[output].as_output())
        if linker_map:
            link_cmd.add(linker_map_args(cxx_toolchain, outputs[linker_map].as_output()).flags)
        if gc_sections_output:
            link_cmd.add(gc_sections_args(cxx_toolchain, outputs[gc_sections_output].as_output()).flags)
        link_cmd.add(link_cmd_parts.post_linker_flags)

        ctx.actions.run(
            link_cmd,
            category = make_cat("thin_lto_link"),
            identifier = identifier,
            prefer_local = link_action_execution_properties.prefer_local,
            prefer_remote = link_action_execution_properties.prefer_remote,
            local_only = link_action_execution_properties.local_only,
            weight = opts.link_weight,
            force_full_hybrid_if_capable = link_action_execution_properties.full_hybrid,
            allow_cache_upload = enable_cache_upload,
        )

    final_link_inputs = [link_plan_out, final_link_index] + archive_opt_manifests
    ctx.actions.dynamic_output(
        dynamic = final_link_inputs,
        inputs = [],
        outputs = [output.as_output()] + ([linker_map.as_output()] if linker_map else []) + ([gc_sections_output.as_output()] if gc_sections_output else []),
        f = thin_lto_final_link,
    )

    external_debug_info = make_artifact_tset(
        label = ctx.label,
        actions = ctx.actions,
        artifacts = objects_external_debug_info,
        children = [unpack_external_debug_info(ctx.actions, link_args) for link_args in links],
    )

    if enable_bolt:
        bolt_output = bolt(ctx, output, external_debug_info, identifier, dwp_tool_available, allow_cache_upload = enable_cache_upload)
        final_output = bolt_output.output
        split_debug_output = bolt_output.dwo_output
    else:
        final_output = output
        split_debug_output = None

    if dwp_tool_available:
        dwp_output = ctx.actions.declare_output(output.short_path.removesuffix(PRE_BOLT_SUFFIX) + ".dwp", has_content_based_path = False)

        def dynamic_run_dwp_action(ctx: AnalysisContext, artifacts, outputs):
            plan = artifacts[link_plan_out].read_json()
            plan_index = {int(k): v for k, v in plan["index"].items()}
            non_lto_objects = {int(k): 1 for k in plan["non_lto_objects"]}

            referenced_objects = list(final_link_inputs)

            # Include hidden dependencies for the final link.
            for idx, link_data in enumerate(link_data_repository):
                if isinstance(link_data, _BitcodeLinkData):
                    if idx in plan_index:
                        referenced_objects.append(link_data.opt_object)
                    elif idx in non_lto_objects:
                        referenced_objects.append(link_data.initial_object)
                elif isinstance(link_data, _ArchiveLinkData):
                    referenced_objects.append(link_data.opt_objects_dir)
                    if link_data.dwo_dir != None:
                        referenced_objects.append(link_data.dwo_dir)
                else:
                    fail("Unexpected link_data type", link_data)

            if split_debug_output:
                referenced_objects += [split_debug_output]
            else:
                referenced_objects += project_artifacts(ctx.actions, external_debug_info)

            run_dwp_action(
                ctx = ctx,
                toolchain = cxx_toolchain,
                obj = final_output,
                identifier = identifier,
                category_suffix = category_suffix,
                referenced_objects = referenced_objects,
                dwp_output = outputs[dwp_output],
                # Mirror the link action's execution preference so dwp runs
                # alongside the link (avoids round-tripping dwo files when link
                # is local) -- unless the caller asked for dwp to be scheduled
                # separately, e.g. because the link is only local for
                # build-info stamping.
                action_execution_properties = dwp_action_execution_properties,
            )

        ctx.actions.dynamic_output(
            dynamic = [link_plan_out],
            inputs = [],
            outputs = [dwp_output.as_output()],
            f = dynamic_run_dwp_action,
        )
    else:
        dwp_output = None

    unstripped_output = final_output
    if opts.strip:
        strip_args = opts.strip_args_factory(ctx) if opts.strip_args_factory else cmd_args()
        final_output = strip_object(ctx, cxx_toolchain, final_output, strip_args, category_suffix, allow_cache_upload = enable_cache_upload)

    final_output = (
        stamp_build_info(
            ctx,
            final_output,
            links = opts.links,
            build_info_json = build_info_json,
        )
        if executable_link
        else final_output
    )

    return LinkedObject(
        output = final_output,
        unstripped_output = unstripped_output,
        prebolt_output = output,
        dwp = dwp_output,
        external_debug_info = external_debug_info,
        linker_argsfile = final_link_index,
        linker_command = None,  # There is no notion of a single linker command for DistLTO
        index_argsfile = index_argsfile_out,
    )
