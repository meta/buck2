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
)
load("@prelude//apple:apple_utility.bzl", "get_apple_architecture")
# @oss-disable[end= ]: load("@prelude//apple/meta_only:shared_library_interfaces.bzl", "get_shared_library_interface_generation_linker_flags")
load(
    "@prelude//apple/swift:swift_incremental_support.bzl",
    "get_uses_content_based_paths",
)
load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load(
    "@prelude//cxx:cxx_link_utility.bzl",
    "CxxSanitizerRuntimeArguments",
    "cxx_link_cmd_parts",
    "get_extra_darwin_linker_flags",
    "linker_map_args",
)
load("@prelude//cxx:cxx_toolchain_types.bzl", "LinkerType")
load("@prelude//cxx:link_types.bzl", "ExtraLinkerOutputCategory", "LinkOptions")
load("@prelude//cxx:target_sdk_version.bzl", "get_target_sdk_version_flags")
load(
    "@prelude//linking:link_info.bzl",
    "ArchiveLinkable",
    "ExtraLinkerOutputs",
    "FrameworksLinkable",  # @unused Used as a type
    "LinkInfo",
    "LinkedObject",
    "ObjectsLinkable",
    "SharedLibLinkable",  # @unused Used as a type
    "SwiftmoduleLinkable",  # @unused Used as a type
    "append_linkable_args",
    "map_to_link_infos",
    "unpack_external_debug_info",
)
load("@prelude//linking:strip.bzl", "strip_object")
load("@prelude//utils:argfile.bzl", "at_argfile")
load("@prelude//utils:arglike.bzl", "ArgLike")
load(":common_types.bzl", "DThinLTOLinkData", "DynamicLibraryLinkData", "EagerBitcodeLinkData", "LazyBitcodeLinkData", "LinkDataType")
load(":thin_link.bzl", "thin_link")
load(":thin_link_record_defs.bzl", "ObjectFileOptimizationPlan")

def identity_projection(value: cmd_args):
    return value

def get_distributed_link_action_identifier(final_binary_out: Artifact) -> str:
    return final_binary_out.short_path + ".dist_lto"

def name_for_obj(distributed_link_action_identifier: str, artifact: Artifact, recorded_outputs: dict[str, int]) -> str:
    """Returns a unique object path qualified by its distributed link and owner."""
    owner = artifact.owner
    if owner != None:
        target = owner.raw_target()
        parts = [distributed_link_action_identifier, "__target__", target.cell]
        if target.package:
            parts.append(target.package)
        parts.extend([target.name, "__artifact__", artifact.short_path])
        prefix = "/".join(parts)
    else:
        prefix = "{}/__ownerless__/{}".format(distributed_link_action_identifier, artifact.short_path)

    case_insensitive_prefix = prefix.lower()
    if case_insensitive_prefix in recorded_outputs:
        recorded_outputs[case_insensitive_prefix] += 1
        extra = recorded_outputs[case_insensitive_prefix]
        prefix = "{}-{}".format(prefix, extra)
    else:
        recorded_outputs[case_insensitive_prefix] = 1
    return prefix

# This tset exists for the sole purpose of saving memory. When action inputs
# are hashed, the memory cost for hashing tset projection generated input
# tree nodes is paid once per unique node. Without a tset projection every
# input is hashed once per action it is included in. The opt actions created
# here share a tremendous number of inputs, so we might end up hashing a
# given input 1000s of times, and paying dearly for it in memory consumption.
# This is not ideal either, the memory allocated to hash these tset projection
# generated inputs will not be freed for the duration of the build, but in
# testing that is a worth while tradeoff.
IdentityTSet = transitive_set(args_projections = {"identity": identity_projection})

def _sort_index_link_data(input_list: list[DThinLTOLinkData]) -> list[DThinLTOLinkData]:
    # Sort link datas to reduce binary size. The idea is to encourage the linker to load the minimal number of object files possible. We load force loaded archives first (since they will be loaded no matter what), then non lazy object files (which will also be loaded no matter what), then shared libraries (to share as many symbols as possible), then finally regular archives
    force_loaded_archives = []
    regular_archives = []
    object_files = []
    dynamic_libraries = []
    for link_data in input_list:
        if link_data.data_type == LinkDataType("eager_bitcode"):
            object_files.append(link_data)
        elif link_data.data_type == LinkDataType("lazy_bitcode"):
            regular_archives.append(link_data)
        elif link_data.data_type == LinkDataType("dynamic_library"):
            dynamic_libraries.append(link_data)

    return force_loaded_archives + object_files + dynamic_libraries + regular_archives

# Generate linker flags that are common to thin-link, native-link, and the link invocation used
# to generate a shared library interface
def _get_common_linker_flags(ctx: AnalysisContext, link_infos: list[LinkInfo], executable_link: bool, sanitizer_runtime_args: list[ArgLike]):
    cxx_toolchain = get_cxx_toolchain_info(ctx)
    result = cmd_args(cxx_link_cmd_parts(cxx_toolchain, executable_link).linker_flags)
    result.add(get_extra_darwin_linker_flags(ctx))
    result.add(sanitizer_runtime_args)

    for link in link_infos:
        result.add(link.pre_flags)
        result.add(link.post_flags)
        for linkable in link.linkables:
            if isinstance(linkable, ArchiveLinkable):
                result.add(cmd_args(hidden = linkable.archive.hidden))

    return result

DoubleCodegenState = enum(
    "disabled",
    "first_round",
    "second_round",
)

def create_output_first_codegen_round_native_object_file(ctx, name, double_codegen_enabled, uses_content_based_paths):
    if double_codegen_enabled:
        return ctx.actions.declare_output(name + ".opt.first_codegen_round.o", has_content_based_path = uses_content_based_paths)
    return None

def create_output_precodegen_ir(ctx, name, double_codegen_enabled, uses_content_based_paths):
    if double_codegen_enabled:
        return ctx.actions.declare_output(name + ".opt.precodegen.bc", has_content_based_path = uses_content_based_paths)
    return None

def complete_distributed_link_with_expanded_archive_link_data(
    ctx: AnalysisContext,
    artifacts,
    outputs,
    common_opt_cmd: cmd_args,
    double_codegen_enabled: bool,
    executable_link: bool,
    external_debug_info_container_directory: Artifact,
    extra_lto_outputs: dict[str, Artifact],
    extra_native_link_outputs: dict[str, Artifact],
    final_binary_out: Artifact,
    index_argsfile_out: Artifact,
    link_infos: list[LinkInfo],
    link_options: LinkOptions,
    linker_argsfile_out: Artifact,
    linker_map_out: Artifact | None,
    make_cat,
    prepared_archive_artifacts,
    sanitizer_runtime_args: CxxSanitizerRuntimeArguments,
    uses_content_based_paths: bool,
):
    cxx_toolchain = get_cxx_toolchain_info(ctx)
    lto_planner = cxx_toolchain.internal_tools.dist_lto.planner[LinkerType("darwin")]
    lto_opt = cxx_toolchain.internal_tools.dist_lto.opt[LinkerType("darwin")]
    lto_archive_mapper = cxx_toolchain.internal_tools.dist_lto.archive_mapper

    distributed_link_action_identifier = get_distributed_link_action_identifier(final_binary_out)
    recorded_outputs = {}

    # The process to optimize and codegen a given bitcode file may depend on other bitcode
    # files (ie. to inline function calls between bitcode files). Which bitcode files depend
    # on one another is not known until thin link (sometimes called the dynamic plan) is run.
    # For each input bitcode file, thin link will produce a plan JSON document that will identify
    # which other bitcode files (or archive of bitcode files) it should be optimized along side.
    # Thin link idenitifies these depended upon bitcode files by their index in the sorted version of this array.
    raw_link_data = []
    debug_info_symlink_map = {}
    deps_linker_flags = cmd_args()

    def declare_extra_opt_outputs(name: str) -> dict[str, Artifact]:
        result = {}
        for output_type_str in extra_lto_outputs.keys():
            result[output_type_str] = ctx.actions.declare_output(name + "." + output_type_str, has_content_based_path = uses_content_based_paths)
        return result

    for link in link_infos:
        deps_linker_flags.add(link.pre_flags)
        deps_linker_flags.add(link.post_flags)

        for linkable in link.linkables:
            if isinstance(linkable, ObjectsLinkable):
                for obj in linkable.objects:
                    name = name_for_obj(distributed_link_action_identifier, obj, recorded_outputs)
                    index_shard_output = ctx.actions.declare_output(name + ".thinlto.bc", has_content_based_path = uses_content_based_paths)
                    plan_output = ctx.actions.declare_output(name + ".opt.plan", has_content_based_path = uses_content_based_paths)
                    final_native_object_file_output = ctx.actions.declare_output(name + ".opt.o", has_content_based_path = uses_content_based_paths)
                    output_first_codegen_round_native_object_file = create_output_first_codegen_round_native_object_file(
                        ctx, name, double_codegen_enabled, uses_content_based_paths
                    )
                    output_precodegen_ir = create_output_precodegen_ir(ctx, name, double_codegen_enabled, uses_content_based_paths)

                    data = DThinLTOLinkData(
                        data_type = LinkDataType("eager_bitcode"),
                        link_data = EagerBitcodeLinkData(
                            name = name,
                            input_object_file = obj,
                            output_index_shard_file = index_shard_output,
                            plan = plan_output,
                            output_final_native_object_file = final_native_object_file_output,
                            output_first_codegen_round_native_object_file = output_first_codegen_round_native_object_file,
                            output_precodegen_ir = output_precodegen_ir,
                            extra_outputs = declare_extra_opt_outputs(name),
                        ),
                    )
                    raw_link_data.append(data)
                    debug_info_symlink_map["debug_info_file_{}".format(len(debug_info_symlink_map))] = final_native_object_file_output
            elif isinstance(linkable, ArchiveLinkable) and linkable.archive.external_objects:
                for virtual_archive_index, obj in enumerate(linkable.archive.external_objects):
                    name = name_for_obj(distributed_link_action_identifier, obj, recorded_outputs)
                    index_shard_output = ctx.actions.declare_output(name + ".thinlto.bc", has_content_based_path = uses_content_based_paths)
                    plan_output = ctx.actions.declare_output(name + ".opt.plan", has_content_based_path = uses_content_based_paths)
                    final_native_object_file_output = ctx.actions.declare_output(name + ".opt.o", has_content_based_path = uses_content_based_paths)
                    output_first_codegen_round_native_object_file = create_output_first_codegen_round_native_object_file(
                        ctx, name, double_codegen_enabled, uses_content_based_paths
                    )
                    output_precodegen_ir = create_output_precodegen_ir(ctx, name, double_codegen_enabled, uses_content_based_paths)

                    # The first member in a non force loaded virtual archive gets --start-lib prended on the command line
                    archive_start = (not linkable.link_whole) and virtual_archive_index == 0
                    archive_end = (not linkable.link_whole) and virtual_archive_index == len(linkable.archive.external_objects) - 1
                    data = DThinLTOLinkData(
                        data_type = LinkDataType("lazy_bitcode"),
                        link_data = LazyBitcodeLinkData(
                            name = name,
                            input_object_file = obj,
                            output_index_shard_file = index_shard_output,
                            plan = plan_output,
                            output_final_native_object_file = final_native_object_file_output,
                            output_first_codegen_round_native_object_file = output_first_codegen_round_native_object_file,
                            output_precodegen_ir = output_precodegen_ir,
                            archive_start = archive_start,
                            archive_end = archive_end,
                            extra_outputs = declare_extra_opt_outputs(name),
                        ),
                    )
                    raw_link_data.append(data)
                    debug_info_symlink_map["debug_info_file_{}".format(len(debug_info_symlink_map))] = final_native_object_file_output
            elif isinstance(linkable, ArchiveLinkable):
                manifest = artifacts[prepared_archive_artifacts[linkable.archive.artifact].manifest].read_json()
                if not len(manifest["objects"]):
                    continue

                archive_name = name_for_obj(
                    distributed_link_action_identifier,
                    linkable.archive.artifact,
                    recorded_outputs,
                )
                archive_contents_mapper_cmd = cmd_args(lto_archive_mapper)
                archive_contents_mapper_cmd.add("--objects_dir", prepared_archive_artifacts[linkable.archive.artifact].extracted_object_files_dir)
                for archive_index, object_basename in enumerate(manifest["objects"]):
                    archive_start = (not linkable.link_whole) and archive_index == 0
                    archive_end = (not linkable.link_whole) and archive_index == len(manifest["objects"]) - 1
                    name = "{}/{}".format(archive_name, object_basename)
                    input_object_file = ctx.actions.declare_output(name, has_content_based_path = uses_content_based_paths)
                    index_shard_output = ctx.actions.declare_output(name + ".thinlto.bc", has_content_based_path = uses_content_based_paths)
                    plan_output = ctx.actions.declare_output(name + ".opt.plan", has_content_based_path = uses_content_based_paths)
                    final_native_object_file_output = ctx.actions.declare_output(name + ".opt.o", has_content_based_path = uses_content_based_paths)
                    output_first_codegen_round_native_object_file = create_output_first_codegen_round_native_object_file(
                        ctx, name, double_codegen_enabled, uses_content_based_paths
                    )
                    output_precodegen_ir = create_output_precodegen_ir(ctx, name, double_codegen_enabled, uses_content_based_paths)

                    raw_link_data.append(
                        DThinLTOLinkData(
                            data_type = LinkDataType("lazy_bitcode"),
                            link_data = LazyBitcodeLinkData(
                                name = name,
                                input_object_file = input_object_file,
                                output_index_shard_file = index_shard_output,
                                plan = plan_output,
                                output_final_native_object_file = final_native_object_file_output,
                                output_first_codegen_round_native_object_file = output_first_codegen_round_native_object_file,
                                output_precodegen_ir = output_precodegen_ir,
                                archive_start = archive_start,
                                archive_end = archive_end,
                                extra_outputs = declare_extra_opt_outputs(name),
                            ),
                        )
                    )
                    archive_contents_mapper_cmd.add("--object_to_map", object_basename, input_object_file.as_output())
                    debug_info_symlink_map["debug_info_file_{}".format(len(debug_info_symlink_map))] = final_native_object_file_output

                    # A native member is linked as extracted, so the debug map names the extracted object.
                    debug_info_symlink_map["debug_info_file_{}".format(len(debug_info_symlink_map))] = input_object_file
                ctx.actions.run(archive_contents_mapper_cmd, category = make_cat("thin_lto_map_archive"), identifier = archive_name)

            elif isinstance(linkable, SharedLibLinkable):
                data = DThinLTOLinkData(
                    data_type = LinkDataType("dynamic_library"),
                    link_data = DynamicLibraryLinkData(linkable = linkable),
                )
                raw_link_data.append(data)
            elif isinstance(linkable, FrameworksLinkable) or isinstance(linkable, SwiftmoduleLinkable):
                # These linkables are handled separately for flag deduplication purposes, as in append_linkable_args:
                # https://www.internalfb.com/code/fbsource/[c6d2c820b394]/fbcode/buck2/prelude/linking/link_info.bzl?lines=271-278
                pass
            else:
                fail("Unhandled linkable type: {}".format(str(linkable)))

    common_link_flags = _get_common_linker_flags(ctx, link_infos, executable_link, sanitizer_runtime_args.extra_link_args)

    sorted_index_link_data = _sort_index_link_data(raw_link_data)

    link_plan_out = ctx.actions.declare_output(final_binary_out.basename + ".link-plan.json", has_content_based_path = uses_content_based_paths)

    thin_link(
        ctx = ctx,
        final_binary_out = final_binary_out,
        sorted_index_link_data = sorted_index_link_data,
        common_link_flags = common_link_flags,
        linker = cxx_link_cmd_parts(cxx_toolchain, executable_link).linker,
        lto_planner = lto_planner,
        post_linker_flags = cxx_link_cmd_parts(cxx_toolchain, executable_link).post_linker_flags,
        index_argsfile_out = outputs[index_argsfile_out],
        link_plan_out = link_plan_out,
        identifier = link_options.identifier,
        use_content_based_output_paths = uses_content_based_paths,
        make_cat = make_cat,
        distributed_link_action_identifier = distributed_link_action_identifier,
    )

    # Create an argsfile and dump all the flags to be processed later by lto_opt.
    # These flags are common to all opt actions, we don't need an argfile for each action, one
    # for the entire link unit will do.
    opt_argsfile = ctx.actions.declare_output(final_binary_out.basename + ".lto_opt_argsfile", has_content_based_path = uses_content_based_paths)
    ctx.actions.write(opt_argsfile.as_output(), common_opt_cmd, allow_args = True)

    def optimize_object(ctx: AnalysisContext, artifacts, outputs, bitcode_link_data, double_codegen_state: DoubleCodegenState, merged_cgdata: Artifact | None):
        output_index_shard_file = bitcode_link_data.output_index_shard_file
        input_object_file = bitcode_link_data.input_object_file
        name = bitcode_link_data.name
        if double_codegen_state == DoubleCodegenState("first_round"):
            output_native_object_file = bitcode_link_data.output_first_codegen_round_native_object_file
        else:
            output_native_object_file = bitcode_link_data.output_final_native_object_file

        plan = bitcode_link_data.plan

        optimization_plan = ObjectFileOptimizationPlan(**artifacts[plan].read_json())

        # If the object was not compiled with thinlto flags, then there
        # won't be valid outputs for it from the indexing, but we still
        # need to bind the artifact. Similarily, if a bitcode file is not
        # loaded by the indexing phase, there is no point optimizing it.
        if not optimization_plan.loaded_by_linker or not optimization_plan.is_bitcode:
            ctx.actions.write(outputs[output_native_object_file].as_output(), "")
            if double_codegen_state == DoubleCodegenState("first_round"):
                ctx.actions.write(outputs[bitcode_link_data.output_precodegen_ir].as_output(), "")
            if double_codegen_state == DoubleCodegenState("disabled") or double_codegen_state == DoubleCodegenState("first_round"):
                mapped_extra_outputs = {output_type: outputs[artifact] for output_type, artifact in bitcode_link_data.extra_outputs.items()}
                for extra_output_artifact in mapped_extra_outputs.values():
                    ctx.actions.write(extra_output_artifact.as_output(), "")
            return

        opt_cmd = cmd_args(lto_opt)
        if double_codegen_state == DoubleCodegenState("second_round"):
            # For the second round, resume from the precodegen IR produced in the first round
            opt_cmd.add("--input", bitcode_link_data.output_precodegen_ir)
            opt_cmd.add("--codegen-only")
        else:
            # First round or disabled, there is nothing to resume from
            opt_cmd.add("--input", input_object_file)

        # Always provide the index regardless of round because codegen depends on information in the index
        opt_cmd.add("--index", output_index_shard_file)

        opt_cmd.add(cmd_args(hidden = common_opt_cmd))
        opt_cmd.add("--shared-args", opt_argsfile)
        opt_cmd.add("--compiler", cxx_toolchain.cxx_compiler_info.compiler)
        if double_codegen_state == DoubleCodegenState("first_round"):
            opt_cmd.add("--generate-cgdata")
        elif double_codegen_state == DoubleCodegenState("second_round"):
            opt_cmd.add("--read-cgdata", merged_cgdata)

        # Second round has already completed importing, so the only bitcode file necessary is the one being optimized,
        # inlined functions have already been embedded into the saved module in round 1
        if double_codegen_state != DoubleCodegenState("second_round"):
            imported_input_bitcode_files = [sorted_index_link_data[idx].link_data.input_object_file for idx in optimization_plan.imports]

            opt_cmd.add(cmd_args(hidden = imported_input_bitcode_files))

        projected_opt_cmd = cmd_args(ctx.actions.tset(IdentityTSet, value = opt_cmd).project_as_args("identity"))

        if double_codegen_state == DoubleCodegenState("first_round"):
            category_string = "thin_lto_opt_object_first_round"
        elif double_codegen_state == DoubleCodegenState("second_round"):
            category_string = "thin_lto_opt_object_second_round"
        else:
            category_string = "thin_lto_opt_object"

        # We have to add outputs after wrapping the cmd_args in a projection
        projected_opt_cmd.add("--out", outputs[output_native_object_file].as_output())

        if double_codegen_state == DoubleCodegenState("first_round"):
            projected_opt_cmd.add("--save-precodegen-ir", outputs[bitcode_link_data.output_precodegen_ir].as_output())

        if link_options.extra_linker_outputs_flags_factory and (
            double_codegen_state == DoubleCodegenState("disabled") or double_codegen_state == DoubleCodegenState("first_round")
        ):
            # Extra linker outputs are all produced during the first round (or only round if double codegen is disabled)
            mapped_extra_outputs = {output_type: outputs[artifact] for output_type, artifact in bitcode_link_data.extra_outputs.items()}
            extra_outputs_args = cmd_args(
                link_options.extra_linker_outputs_flags_factory(
                    ctx,
                    mapped_extra_outputs,
                    ExtraLinkerOutputCategory("produced-during-distributed-thin-lto-opt"),
                ),
            )
            projected_opt_cmd.add("--", extra_outputs_args)

        ctx.actions.run(projected_opt_cmd, category = make_cat(category_string), identifier = name)

    def dynamic_optimize(bitcode_link_data, double_codegen_state: DoubleCodegenState, merged_cgdata: Artifact | None = None):
        if double_codegen_state == DoubleCodegenState("first_round"):
            optimize_object_outputs = [
                bitcode_link_data.output_first_codegen_round_native_object_file.as_output(),
                bitcode_link_data.output_precodegen_ir.as_output(),
            ]

            # Produce extra linker outputs in first round
            optimize_object_outputs += [artifact.as_output() for artifact in bitcode_link_data.extra_outputs.values()]
        elif double_codegen_state == DoubleCodegenState("second_round"):
            # Only produce the final native object file in the second round
            optimize_object_outputs = [bitcode_link_data.output_final_native_object_file.as_output()]
        else:
            # If there is only one around, produce everything in that round
            optimize_object_outputs = [bitcode_link_data.output_final_native_object_file.as_output()]
            optimize_object_outputs += [artifact.as_output() for artifact in bitcode_link_data.extra_outputs.values()]

        ctx.actions.dynamic_output(
            dynamic = [bitcode_link_data.plan],
            inputs = [],
            outputs = optimize_object_outputs,
            f = lambda ctx, artifacts, outputs: optimize_object(
                ctx,
                artifacts,
                outputs,
                bitcode_link_data,
                double_codegen_state = double_codegen_state,
                merged_cgdata = merged_cgdata,
            ),
        )

    for artifact in sorted_index_link_data:
        if artifact.data_type == LinkDataType("eager_bitcode") or artifact.data_type == LinkDataType("lazy_bitcode"):
            double_codegen_state = DoubleCodegenState("first_round") if double_codegen_enabled else DoubleCodegenState("disabled")
            dynamic_optimize(artifact.link_data, double_codegen_state = double_codegen_state, merged_cgdata = None)

    def merge_cgdata(ctx: AnalysisContext, artifacts, outputs, output_merged_cgdata_file):
        merge_cmd = cmd_args(cxx_toolchain.llvm_cgdata)
        merge_args = cmd_args("--merge")
        for artifact in sorted_index_link_data:
            if artifact.data_type == LinkDataType("eager_bitcode") or artifact.data_type == LinkDataType("lazy_bitcode"):
                optimization_plan = ObjectFileOptimizationPlan(**artifacts[artifact.link_data.plan].read_json())
                if not optimization_plan.loaded_by_linker or not optimization_plan.is_bitcode:
                    continue

                # Some targets have spaces in them, we wrap in single quotes to make sure they are treated as single files
                merge_args.add(cmd_args(artifact.link_data.output_first_codegen_round_native_object_file, quote = "shell"))
        merge_args.add("--output", outputs[output_merged_cgdata_file].as_output())
        merge_cmd.add(
            at_argfile(
                actions = ctx.actions,
                name = final_binary_out.basename + ".llvm-cgdata-argsfile",
                args = merge_args,
            )
        )
        ctx.actions.run(merge_cmd, category = make_cat("thin_lto_merge_cgdata"), identifier = link_options.identifier)

    if double_codegen_enabled:
        merged_cgdata_file = ctx.actions.declare_output(final_binary_out.basename + ".cgdata", has_content_based_path = uses_content_based_paths)
        ctx.actions.dynamic_output(
            dynamic = [
                artifact.link_data.plan
                for artifact in sorted_index_link_data
                if artifact.data_type == LinkDataType("lazy_bitcode") or artifact.data_type == LinkDataType("eager_bitcode")
            ],
            inputs = [],
            outputs = [merged_cgdata_file.as_output()],
            f = lambda ctx, artifacts, outputs: merge_cgdata(
                ctx,
                artifacts,
                outputs,
                merged_cgdata_file,
            ),
        )

        for artifact in sorted_index_link_data:
            link_data = artifact.link_data
            if artifact.data_type == LinkDataType("eager_bitcode") or artifact.data_type == LinkDataType("lazy_bitcode"):
                dynamic_optimize(link_data, merged_cgdata = merged_cgdata_file, double_codegen_state = DoubleCodegenState("second_round"))

    ctx.actions.symlinked_dir(outputs[external_debug_info_container_directory], debug_info_symlink_map)

    # merge extra outputs produced by opt actions
    def merge_extra_opt_outputs(ctx: AnalysisContext, artifacts, outputs, lto_outputs):
        opt_outputs_to_merge = []
        for artifact in sorted_index_link_data:
            if artifact.data_type == LinkDataType("eager_bitcode") or artifact.data_type == LinkDataType("lazy_bitcode"):
                optimization_plan = ObjectFileOptimizationPlan(**artifacts[artifact.link_data.plan].read_json())
                if not optimization_plan.loaded_by_linker or not optimization_plan.is_bitcode:
                    continue

                opt_outputs_to_merge.append(artifact.link_data.extra_outputs)

        mapped_lto_outputs = {output_type: outputs[artifact] for output_type, artifact in lto_outputs.items()}
        link_options.extra_distributed_thin_lto_opt_outputs_merger(ctx, mapped_lto_outputs, opt_outputs_to_merge)

    if link_options.extra_distributed_thin_lto_opt_outputs_merger:
        if extra_lto_outputs:
            mapped_lto_outputs = {output_type: outputs[artifact] for output_type, artifact in extra_lto_outputs.items()}
            ctx.actions.dynamic_output(
                dynamic = [
                    artifact.link_data.plan
                    for artifact in sorted_index_link_data
                    if artifact.data_type == LinkDataType("lazy_bitcode") or artifact.data_type == LinkDataType("eager_bitcode")
                ],
                inputs = [],
                outputs = [outputs[artifact].as_output() for artifact in extra_lto_outputs.values()],
                f = lambda ctx, artifacts, outputs: merge_extra_opt_outputs(ctx, artifacts, outputs, mapped_lto_outputs),
            )

    def thin_lto_final_link(ctx: AnalysisContext, artifacts, outputs):
        plan = artifacts[link_plan_out].read_json()
        link_args = cmd_args()

        # non_lto_objects are the ones that weren't compiled with thinlto
        # flags. In that case, we need to link against the original object.
        non_lto_objects = {int(k): 1 for k in plan["non_lto_objects"]}

        object_file_linker_inputs = []

        for idx, artifact in enumerate(sorted_index_link_data):
            if artifact.data_type == LinkDataType("dynamic_library"):
                append_linkable_args(link_args, artifact.link_data.linkable)
            elif artifact.data_type == LinkDataType("eager_bitcode") or artifact.data_type == LinkDataType("lazy_bitcode"):
                optimization_plan = ObjectFileOptimizationPlan(**artifacts[artifact.link_data.plan].read_json())

                # Object files that are not loaded are not included in the native link.
                if not optimization_plan.loaded_by_linker:
                    continue

                link_line_position = optimization_plan.final_link_line_position

                if idx in non_lto_objects:
                    object_file_linker_inputs.append((link_line_position, artifact.link_data.input_object_file))
                else:
                    object_file_linker_inputs.append((link_line_position, artifact.link_data.output_final_native_object_file))

        link_cmd_parts = cxx_link_cmd_parts(cxx_toolchain, executable_link)
        link_cmd = cmd_args(link_cmd_parts.linker)
        link_args.add(common_link_flags)

        if link_options.extra_linker_outputs_flags_factory != None:
            # We need the inner artifacts here
            mapped_outputs = {output_type: outputs[artifact] for output_type, artifact in extra_native_link_outputs.items()}
            link_args.add(
                link_options.extra_linker_outputs_flags_factory(
                    ctx, mapped_outputs, ExtraLinkerOutputCategory("produced-during-distributed-thin-lto-native-link")
                )
            )

        link_args.add("-o", outputs[final_binary_out].as_output())
        if linker_map_out:
            link_args.add(linker_map_args(cxx_toolchain, outputs[linker_map_out].as_output()).flags)
        ordered_object_file_linker_inputs = sorted(object_file_linker_inputs, key = lambda x: x[0])
        for _, artifact in ordered_object_file_linker_inputs:
            link_args.add(artifact)
        link_cmd.add(
            at_argfile(
                actions = ctx.actions,
                name = outputs[linker_argsfile_out],
                args = link_args,
                allow_args = True,
                has_content_based_path = uses_content_based_paths,
            )
        )
        link_cmd.add(link_cmd_parts.post_linker_flags)

        ctx.actions.run(link_cmd, category = make_cat("thin_lto_link"), identifier = link_options.identifier)

    final_link_outputs = [outputs[final_binary_out].as_output(), outputs[linker_argsfile_out].as_output()]
    if linker_map_out:
        final_link_outputs.append(outputs[linker_map_out].as_output())

    # Add outputs produced by native link only
    final_link_outputs += [outputs[output_artifact].as_output() for output_artifact in extra_native_link_outputs.values()]

    ctx.actions.dynamic_output(
        dynamic = [link_plan_out]
        + [
            artifact.link_data.plan
            for artifact in sorted_index_link_data
            if artifact.data_type == LinkDataType("lazy_bitcode") or artifact.data_type == LinkDataType("eager_bitcode")
        ],
        inputs = [],
        outputs = final_link_outputs,
        f = thin_lto_final_link,
    )

PreparedArchiveArtitfacts = record(
    manifest = field(Artifact),
    extracted_object_files_dir = field(Artifact),
)

def generate_shared_library_interface(
    ctx: AnalysisContext,
    identifier: str,
    executable_link: bool,
    link_infos: list[LinkInfo],
    output: Artifact,
    sanitizer_runtime_args: list[ArgLike],
    shared_library_interface_out: Artifact,
):
    # This flag generation logic must be at least semmantically equivalent to the logic used to create a thin-link invocation.
    # Do not for example change the input order in one place, they must be kept in sync to generate an accurate interface.

    common_linker_flags = _get_common_linker_flags(ctx, link_infos, executable_link, sanitizer_runtime_args)

    eager_objects = cmd_args()
    lazy_objects = cmd_args()
    dynamic_libraries = cmd_args()

    for link in link_infos:
        for linkable in link.linkables:
            if isinstance(linkable, ObjectsLinkable):
                for obj in linkable.objects:
                    eager_objects.add(obj)
            elif isinstance(linkable, ArchiveLinkable) and linkable.archive.external_objects:
                lazy_objects.add("-Wl,--start-lib")
                for obj in linkable.archive.external_objects:
                    lazy_objects.add(obj)
                lazy_objects.add("-Wl,--end-lib")
            elif isinstance(linkable, ArchiveLinkable):
                # Thin link requires that we extract all archives beforehand via dynamic output so that we can declare various
                # artifacts for each member. These artifacts are the link plan, the summary shard etc.
                # However, this linking invocation does not produce any of those, it only produces the shared library interface.
                # Passing the archive directly instead of extracting it, then passing the members between --start-lib --end-lib
                # flags should be semantically equivalent. Note however, that we only do this in this branch where we are dealing
                # with some kind of pre-build static archive where Buck is not aware of the contents. If we built this static archive,
                # just pass the members directly above, don't bother creating an archive for no reason.
                lazy_objects.add(linkable.archive.artifact)
            elif isinstance(linkable, SharedLibLinkable):
                append_linkable_args(dynamic_libraries, linkable)
            elif isinstance(linkable, FrameworksLinkable) or isinstance(linkable, SwiftmoduleLinkable):
                # These linkables are handled separately for flag deduplication purposes, as in append_linkable_args:
                # https://www.internalfb.com/code/fbsource/[c6d2c820b394]/fbcode/buck2/prelude/linking/link_info.bzl?lines=271-278
                pass
            else:
                fail("Unhandled linkable type: {}".format(str(linkable)))

    interface_generation_linker_args = cmd_args()
    interface_generation_linker_args.add(common_linker_flags)
    interface_generation_linker_args.add(eager_objects)
    interface_generation_linker_args.add(lazy_objects)
    interface_generation_linker_args.add(dynamic_libraries)
    output_as_string = cmd_args(output, ignore_artifacts = True)
    interface_generation_linker_args.add("-o", output_as_string)
    interface_generation_linker_argsfile = ctx.actions.declare_output(
        output.basename + ".shared_library_interface_generation_argsfile", has_content_based_path = False
    )
    interface_generation_linker_argsfile, _ = ctx.actions.write(
        interface_generation_linker_argsfile,
        interface_generation_linker_args,
        with_inputs = True,
        allow_args = True,
    )
    cxx_toolchain = get_cxx_toolchain_info(ctx)
    linker = cxx_link_cmd_parts(cxx_toolchain, executable_link).linker
    interface_generation_linker_invocation = cmd_args(linker)
    interface_generation_linker_invocation.add(cmd_args(interface_generation_linker_argsfile, format = "@{}"))

    interface_generation_linker_invocation.add(
        # @oss-disable[end= ]: get_shared_library_interface_generation_linker_flags(shared_library_interface_out),
        cmd_args(),
    )
    ctx.actions.run(interface_generation_linker_invocation, category = "generate_shared_library_interface", identifier = identifier)

def cxx_darwin_dist_link(
    ctx: AnalysisContext,
    # The destination for the link output.
    output: Artifact,
    opts: LinkOptions,
    double_codegen_enabled: bool,
    executable_link: bool,
    sanitizer_runtime_args: CxxSanitizerRuntimeArguments,
    shared_library_interface: Artifact | None,
    linker_map: Artifact | None = None,
) -> (LinkedObject, dict[str, list[DefaultInfo]]):
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

    def make_cat(c: str) -> str:
        """Used to make sure categories for our actions include the provided suffix"""
        if opts.category_suffix != None:
            return c + "_" + opts.category_suffix
        return c

    links = opts.links + opts.binary_links

    link_infos = map_to_link_infos(links)

    cxx_toolchain = get_cxx_toolchain_info(ctx)
    lto_prepare = cxx_toolchain.internal_tools.dist_lto.prepare[LinkerType("darwin")]

    archive_manifests = []

    prepare_cat = make_cat("thin_lto_prepare")

    prepared_archive_artifacts = {}

    uses_content_based_paths = get_uses_content_based_paths(ctx)

    distributed_link_action_identifier = get_distributed_link_action_identifier(output)
    recorded_outputs = {}
    for link in link_infos:
        for linkable in link.linkables:
            if isinstance(linkable, ArchiveLinkable) and not linkable.archive.external_objects:
                archive_name = name_for_obj(
                    distributed_link_action_identifier,
                    linkable.archive.artifact,
                    recorded_outputs,
                )

                archive_manifest = ctx.actions.declare_output(
                    "%s/%s/manifest.json" % (prepare_cat, archive_name), has_content_based_path = uses_content_based_paths
                )
                archive_objects = ctx.actions.declare_output(
                    "%s/%s/objects" % (prepare_cat, archive_name), has_content_based_path = uses_content_based_paths, dir = True
                )

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
                    archive_name,
                    "--target-architecture",
                    get_apple_architecture(ctx),
                    "--lipo",
                    cxx_toolchain.lipo,
                ])
                ctx.actions.run(prepare_args, category = make_cat("thin_lto_prepare"), identifier = archive_name)
                prepared_archive_artifacts[linkable.archive.artifact] = PreparedArchiveArtitfacts(
                    manifest = archive_manifest, extracted_object_files_dir = archive_objects
                )
                archive_manifests.append(archive_manifest)

    linker_argsfile_out = ctx.actions.declare_output(output.basename + ".thinlto_link_argsfile", has_content_based_path = uses_content_based_paths)
    index_argsfile_out = ctx.actions.declare_output(output.basename + ".thinlto_index_argsfile", has_content_based_path = uses_content_based_paths)

    def prepare_opt_flags(link_infos: list[LinkInfo]) -> cmd_args:
        opt_flags = cmd_args(cxx_toolchain.linker_info.dist_thin_lto_codegen_flags)
        if cxx_toolchain.pass_plugin:
            opt_flags.add(cmd_args(cxx_toolchain.pass_plugin, format = "-fpass-plugin={}"))
        opt_flags.add(get_target_sdk_version_flags(ctx))
        for link in link_infos:
            opt_flags.add(link.dist_thin_lto_codegen_flags)
        return opt_flags

    common_opt_cmd = prepare_opt_flags(link_infos)

    external_debug_info_container_directory = ctx.actions.declare_output(
        output.basename + ".debug_info_dir", dir = True, has_content_based_path = uses_content_based_paths
    )

    extra_linker_outputs_factory = opts.extra_linker_outputs_factory
    link_outputs = [output.as_output(), index_argsfile_out.as_output(), linker_argsfile_out.as_output(), external_debug_info_container_directory.as_output()]

    lto_outputs = ExtraLinkerOutputs()
    native_link_outputs = ExtraLinkerOutputs()
    if extra_linker_outputs_factory:
        lto_outputs = extra_linker_outputs_factory(ctx, ExtraLinkerOutputCategory("produced-during-distributed-thin-lto-opt"))
        native_link_outputs = extra_linker_outputs_factory(ctx, ExtraLinkerOutputCategory("produced-during-distributed-thin-lto-native-link"))

    link_outputs.extend([lto_output.as_output() for lto_output in lto_outputs.artifacts.values()])
    link_outputs.extend([native_link_output.as_output() for native_link_output in native_link_outputs.artifacts.values()])

    if linker_map:
        link_outputs.append(linker_map.as_output())

    if shared_library_interface != None:
        generate_shared_library_interface(
            ctx = ctx,
            identifier = opts.identifier,
            executable_link = executable_link,
            link_infos = link_infos,
            output = output,
            sanitizer_runtime_args = sanitizer_runtime_args.extra_link_args,
            shared_library_interface_out = shared_library_interface,
        )

    ctx.actions.dynamic_output(
        dynamic = archive_manifests,
        inputs = [],
        outputs = link_outputs,
        f = lambda ctx, artifacts, outputs: complete_distributed_link_with_expanded_archive_link_data(
            ctx,
            artifacts,
            outputs,
            common_opt_cmd = common_opt_cmd,
            executable_link = executable_link,
            external_debug_info_container_directory = external_debug_info_container_directory,
            extra_lto_outputs = lto_outputs.artifacts,
            extra_native_link_outputs = native_link_outputs.artifacts,
            final_binary_out = output,
            index_argsfile_out = index_argsfile_out,
            link_infos = link_infos,
            link_options = opts,
            linker_argsfile_out = linker_argsfile_out,
            linker_map_out = linker_map,
            make_cat = make_cat,
            double_codegen_enabled = double_codegen_enabled,
            prepared_archive_artifacts = prepared_archive_artifacts,
            sanitizer_runtime_args = sanitizer_runtime_args,
            uses_content_based_paths = uses_content_based_paths,
        ),
    )

    final_output = output
    unstripped_output = output
    if opts.strip:
        strip_args = opts.strip_args_factory(ctx) if opts.strip_args_factory else cmd_args()
        final_output = strip_object(ctx, cxx_toolchain, final_output, strip_args, opts.category_suffix)

    external_debug_info = make_artifact_tset(
        actions = ctx.actions,
        artifacts = [external_debug_info_container_directory],
        label = ctx.label,
        children = [unpack_external_debug_info(ctx.actions, link_args) for link_args in links],
    )

    return LinkedObject(
        output = final_output,
        unstripped_output = unstripped_output,
        prebolt_output = output,
        dwp = None,
        external_debug_info = external_debug_info,
        linker_argsfile = linker_argsfile_out,
        linker_command = None,  # There is no notion of a single linker command for DistLTO
        index_argsfile = index_argsfile_out,
    ), (lto_outputs.providers | native_link_outputs.providers)
