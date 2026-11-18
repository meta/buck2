# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactTSet",
    "make_artifact_tset",
    "project_artifacts",
)
load("@prelude//:paths.bzl", "paths")
# @oss-disable[end= ]: load("@prelude//apple/meta_only:shared_library_interfaces.bzl", "get_shared_library_interface_generation_linker_flags")
load(
    "@prelude//cxx:cxx_bolt.bzl",
    "bolt",
    "cxx_use_bolt",
)
load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "CxxToolchainInfo",
    "LinkerType",
    "ShlibInterfacesMode",
)
load(
    "@prelude//cxx/dist_lto:dist_lto.bzl",
    "cxx_gnu_dist_link",
)
load(
    "@prelude//cxx/dist_lto/darwin:dist_lto.bzl",
    "cxx_darwin_dist_link",
)
load("@prelude//linking:add_elf_sections.bzl", "add_elf_sections_to_executable")
load("@prelude//linking:execution_preference.bzl", "LinkExecutionPreference", "LinkExecutionPreferenceInfo", "get_action_execution_attributes")
load(
    "@prelude//linking:link_info.bzl",
    "ArchiveLinkable",
    "ExtraLinkerOutputs",
    "LinkArgs",
    "LinkedObject",
    "ObjectsLinkable",
    "unpack_external_debug_info",
    "unpack_link_args",
)
load(
    "@prelude//linking:lto.bzl",
    "LtoMode",
    "get_split_debug_lto_info",
)
load("@prelude//linking:stamp_build_info.bzl", "cxx_stamp_build_info", "stamp_build_info")
load("@prelude//linking:strip.bzl", "strip_object")
load("@prelude//utils:actions.bzl", "ActionExecutionAttributes")
load("@prelude//utils:expect.bzl", "expect")
load(
    ":anon_link.bzl",
    "ANON_ATTRS",
    "deserialize_anon_attrs",
    "serialize_anon_attrs",
)
load(":bitcode.bzl", "make_bitcode_bundle")
load(":cxx_context.bzl", "get_cxx_toolchain_info")
load(":cxx_library_utility.bzl", "EMPTY_DEFAULT_INFO")
load(
    ":cxx_link_utility.bzl",
    "LinkArgsOutput",
    "cxx_link_cmd_parts",
    "cxx_runtime_library_arguments",
    "cxx_sanitizer_runtime_arguments",
    "gc_sections_args",
    "generates_split_debug",
    "linker_map_args",
    "linker_supports_linker_maps",
    "make_link_args",
)
load(":debug.bzl", "SplitDebugMode")
load(":dwp.bzl", "dwp", "dwp_available")
load(":hip_debug_extract.bzl", "PRE_EXTRACT_SUFFIX", "hip_debug_extract_available")
load(":link_types.bzl", "CxxLinkResultType", "LinkOptions", "get_dwp_execution_preference", "merge_link_options")
load(
    ":linker.bzl",
    "SharedLibraryFlagOverrides",  # @unused Used as a type
    "get_deffile_flags",
    "get_import_library",
    "get_output_flags",
    "get_shared_library_flags",
    "get_shared_library_name_linker_flags",
)
load(
    ":shared_library_interface.bzl",
    "run_shared_library_interface_producer",
    "shared_library_interface_path",
    "shared_library_interface_producer",
)

CxxLinkerMapData = record(
    map = field(Artifact),
    binary = field(Artifact),
)

CxxGcSectionsData = record(
    gc_sections = field(Artifact),
    binary = field(Artifact),
)

CxxLinkResult = record(
    # The resulting artifact from the link
    linked_object = LinkedObject,
    linker_map_data = [CxxLinkerMapData, None],
    gc_sections_data = [CxxGcSectionsData, None],
    link_execution_preference_info = LinkExecutionPreferenceInfo,
    # A list of runtime shared libraries
    sanitizer_runtime_files = field(list[Artifact]),
    # A dictionary of extra linker outputs generated from
    # the extra_linker_outputs_factory
    extra_outputs = field(dict[str, list[DefaultInfo]], default = {}),
    # A representation of a shared library to link against, rather than the shared library
    # itself. This can be advantageous because the interface is faster to generate,
    # or because it hides implementation changes to the shared library that would cause
    # unnecessary build invalidation.
    shared_library_interface = [Artifact, None],
)

def link_external_debug_info(
    ctx: AnalysisContext, links: list[LinkArgs], split_debug_output: Artifact | None = None, pdb: Artifact | None = None
) -> ArtifactTSet:
    external_debug_artifacts = []

    # When using LTO+split-dwarf, the link step will generate externally
    # referenced debug info.
    if split_debug_output != None:
        external_debug_artifacts.append(split_debug_output)

    if pdb != None:
        external_debug_artifacts.append(pdb)

    external_debug_infos = []

    # Add-in an externally referenced debug info that the linked object may
    # reference (and which may need to be available for debugging).
    for link in links:
        external_debug_infos.append(unpack_external_debug_info(ctx.actions, link))

    return make_artifact_tset(
        actions = ctx.actions,
        label = ctx.label,
        artifacts = external_debug_artifacts,
        children = external_debug_infos,
    )

def link_args_have_hip_device_debug(links: list[LinkArgs]) -> bool:
    for link in links:
        if link.infos != None:
            for info in link.infos:
                if info.has_hip_device_debug:
                    return True
        if link.tset != None:
            for infos in link.tset.infos.traverse():
                if infos.default.has_hip_device_debug:
                    return True
                if infos.stripped != None and infos.stripped.has_hip_device_debug:
                    return True
    return False

def collect_hip_debug_from_links(links: list[LinkArgs]) -> dict[str, list[Artifact]]:
    result = {}
    for link in links:
        if link.infos != None:
            for info in link.infos:
                for arch, files in info.hip_arch_debug_files.items():
                    result.setdefault(arch, [])
                    result[arch].extend(files)
        if link.tset != None:
            for infos in link.tset.infos.traverse():
                for info in [infos.default] + ([infos.stripped] if infos.stripped != None else []):
                    for arch, files in info.hip_arch_debug_files.items():
                        result.setdefault(arch, [])
                        result[arch].extend(files)
    return result

# Actually perform a link into the supplied output.
def cxx_link_into(
    ctx: AnalysisContext,
    # The destination for the link output.
    output: Artifact,
    result_type: CxxLinkResultType,
    opts: LinkOptions,
    output_has_content_based_path: bool = False,
    build_info_json: Artifact | None = None,
    link_groups_enabled: bool = False,
) -> CxxLinkResult:
    cxx_toolchain_info = opts.cxx_toolchain or get_cxx_toolchain_info(ctx)
    linker_info = cxx_toolchain_info.linker_info
    is_incremental_link = opts.incremental_link

    dwp_tool_available = dwp_available(cxx_toolchain_info)
    is_result_executable = result_type.value == "executable"

    if linker_info.generate_linker_maps and linker_supports_linker_maps(linker_info.type):
        linker_map = ctx.actions.declare_output(output.short_path + "-LinkMap.txt", has_content_based_path = False)
        linker_map_data = CxxLinkerMapData(
            map = linker_map,
            binary = output,
        )
    else:
        linker_map = None
        linker_map_data = None

    if linker_info.generate_gc_sections and linker_info.type == LinkerType("gnu"):
        gc_sections_output = ctx.actions.declare_output(output.short_path + "-gc-sections.json", has_content_based_path = False)
        gc_sections_data = CxxGcSectionsData(
            gc_sections = gc_sections_output,
            binary = output,
        )
    else:
        gc_sections_output = None
        gc_sections_data = None

    # Only `stub_from_linker_invocation` has the linker emit the interface; in
    # every other mode it is derived from the linked output, within the link
    # action.
    linker_emits_shared_library_interface = linker_info.type == LinkerType("darwin") and linker_info.shlib_interfaces == ShlibInterfacesMode(
        "stub_from_linker_invocation"
    )
    if not opts.produce_shared_library_interface:
        shared_library_interface = None
    elif linker_emits_shared_library_interface:
        shared_library_interface = ctx.actions.declare_output(output.short_path + ".tbd", has_content_based_path = False)
    else:
        shared_library_interface = ctx.actions.declare_output(shared_library_interface_path(output.short_path), has_content_based_path = True)

    if is_incremental_link:
        ilk_filename = paths.replace_extension(output.short_path, ".ilk")
        ilk_artifact = ctx.actions.declare_output(ilk_filename, has_content_based_path = output_has_content_based_path)
    else:
        ilk_artifact = None

    if linker_info.supports_distributed_thinlto and opts.enable_distributed_thinlto:
        if opts.stderr_output != None:
            fail("stderr_output is only supported for local link actions")
        if not linker_info.lto_mode == LtoMode("thin"):
            fail("Cannot use distributed thinlto if the cxx toolchain doesn't use thin-lto lto_mode")
        if linker_info.runtime_library_files:
            fail("runtime_library_files is not supported with distributed thin-lto")
        sanitizer_runtime_args = cxx_sanitizer_runtime_arguments(ctx, cxx_toolchain_info, output)

        linker_type = linker_info.type
        if linker_type == LinkerType("darwin"):
            exe, extra_outputs = cxx_darwin_dist_link(
                ctx,
                output,
                opts,
                linker_info.thin_lto_premerger_enabled,
                linker_info.thin_lto_double_codegen_enabled,
                is_result_executable,
                sanitizer_runtime_args,
                shared_library_interface if linker_emits_shared_library_interface else None,
                linker_map,
            )
        elif linker_type == LinkerType("gnu"):
            if sanitizer_runtime_args.extra_link_args or sanitizer_runtime_args.sanitizer_runtime_files:
                fail("Cannot use GNU distributed thinlto with sanitizer runtime")
            exe = cxx_gnu_dist_link(
                ctx,
                output,
                opts,
                linker_map,
                gc_sections_output,
                dwp_tool_available,
                is_result_executable,
                build_info_json,
                link_groups_enabled,
            )
            extra_outputs = {}
        else:
            fail("Linker type {} not supported for distributed thin-lto".format(linker_type))

        # A distributed link has no single link command to wrap.
        if shared_library_interface != None and not linker_emits_shared_library_interface:
            run_shared_library_interface_producer(
                ctx = ctx,
                linker_info = linker_info,
                shared_lib = exe.output,
                interface = shared_library_interface,
                identifier = opts.identifier,
            )

        return CxxLinkResult(
            linked_object = exe,
            linker_map_data = linker_map_data,
            gc_sections_data = gc_sections_data,
            link_execution_preference_info = LinkExecutionPreferenceInfo(
                preference = opts.link_execution_preference,
            ),
            sanitizer_runtime_files = sanitizer_runtime_args.sanitizer_runtime_files,
            extra_outputs = extra_outputs,
            shared_library_interface = shared_library_interface,
        )

    link_cmd_parts = cxx_link_cmd_parts(cxx_toolchain_info, is_result_executable)
    extra_linker_outputs = opts.extra_linker_outputs_factory(ctx) if opts.extra_linker_outputs_factory != None else ExtraLinkerOutputs()
    split_debug_output = None
    split_debug_lto_info = get_split_debug_lto_info(ctx.actions, cxx_toolchain_info, output.short_path)
    if split_debug_lto_info != None:
        split_debug_output = split_debug_lto_info.output
    expect(not generates_split_debug(cxx_toolchain_info) or split_debug_output != None)
    sanitizer_runtime_args = cxx_sanitizer_runtime_arguments(ctx, cxx_toolchain_info, output)
    runtime_library_args = cxx_runtime_library_arguments(cxx_toolchain_info)

    def create_local_linker_invocation(add_linker_outputs: bool) -> LinkArgsOutput:
        if linker_map != None and add_linker_outputs:
            links_with_linker_map = opts.links + opts.binary_links + [linker_map_args(cxx_toolchain_info, linker_map.as_output())]
        else:
            links_with_linker_map = opts.links + opts.binary_links

        # Add gc-sections output args if enabled
        if gc_sections_output != None and add_linker_outputs:
            links_with_extra_args = links_with_linker_map + [gc_sections_args(cxx_toolchain_info, gc_sections_output.as_output())]
        else:
            links_with_extra_args = links_with_linker_map

        all_link_args = cmd_args(link_cmd_parts.linker_flags)
        if add_linker_outputs:
            all_link_args.add(get_output_flags(linker_info.type, output))
            if is_incremental_link:
                all_link_args.add("/INCREMENTAL")
            elif linker_info.type == LinkerType("windows"):
                all_link_args.add("/INCREMENTAL:NO")

        if add_linker_outputs:
            # Add the linker args required for any extra linker outputs requested
            if len(extra_linker_outputs.artifacts) > 0:
                if opts.extra_linker_outputs_flags_factory == None:
                    fail("Extra outputs requested but missing flag factory")

                all_link_args.add(opts.extra_linker_outputs_flags_factory(ctx, extra_linker_outputs.artifacts))

            # Darwin LTO requires extra link outputs to preserve debug info
            if split_debug_lto_info != None:
                all_link_args.add(split_debug_lto_info.linker_flags)

        link_args_output = make_link_args(
            ctx,
            ctx.actions,
            cxx_toolchain_info,
            links_with_extra_args,
            output_short_path = output.short_path,
            link_ordering = opts.link_ordering,
            has_content_based_path = output_has_content_based_path,
        )
        all_link_args.add(link_args_output.link_args)

        # Sanitizer runtime args must appear at the end because it can affect
        # behavior of Swift runtime loading when the app also has an embedded
        # Swift runtime.
        all_link_args.add(sanitizer_runtime_args.extra_link_args)

        # Runtime libraries the toolchain provides (e.g. compiler-rt builtins) go at
        # the end, to match the Clang driver's placement of compiler-rt.
        all_link_args.add(runtime_library_args)

        if linker_info.thin_lto_double_codegen_enabled:
            # This flag should only be passed to the toolchain when using local thin-lto,
            # for distributed thin-lto the double codegen is handled differently.
            all_link_args.add("-Wl,-mllvm,-codegen-data-thinlto-two-rounds")

        all_link_args.add(link_cmd_parts.post_linker_flags)

        if linker_info.type == LinkerType("windows"):
            shell_quoted_args = cmd_args(all_link_args)
        else:
            shell_quoted_args = cmd_args(all_link_args, quote = "shell")

        return LinkArgsOutput(link_args = shell_quoted_args, hidden = link_args_output.hidden, pdb_artifact = link_args_output.pdb_artifact)

    link_unit_generation_link_args = create_local_linker_invocation(add_linker_outputs = True)
    argfile, _ = ctx.actions.write(
        output.short_path + ".cxx_link_argsfile",
        link_unit_generation_link_args.link_args,
        allow_args = True,
        with_inputs = True,
        has_content_based_path = cxx_toolchain_info.cxx_compiler_info.supports_content_based_paths,
    )

    if shared_library_interface != None and linker_emits_shared_library_interface:
        # If we ask the linker to produce a shared library interface, it won't produce any other outputs
        # so we shouldn't declare any extra outputs.
        shared_library_interface_generation_linker_args = create_local_linker_invocation(add_linker_outputs = False).link_args
        output_as_string = cmd_args(get_output_flags(linker_info.type, output), ignore_artifacts = True)
        shared_library_interface_generation_argfile, _ = ctx.actions.write(
            output.short_path + ".cxx_shared_library_interface_generation_argsfile",
            cmd_args(shared_library_interface_generation_linker_args, output_as_string),
            allow_args = True,
            with_inputs = True,
            has_content_based_path = cxx_toolchain_info.cxx_compiler_info.supports_content_based_paths,
        )
        link_cmd_parts = cxx_link_cmd_parts(cxx_toolchain_info, is_result_executable)
        shared_library_interface_generation_command = cmd_args(
            link_cmd_parts.linker,
            cmd_args(shared_library_interface_generation_argfile, format = "@{}"),
        )

        shared_library_interface_generation_command.add(
            # @oss-disable[end= ]: get_shared_library_interface_generation_linker_flags(shared_library_interface),
            cmd_args(),
        )
        ctx.actions.run(
            shared_library_interface_generation_command,
            category = "generate_shared_library_interface",
            identifier = opts.identifier,
        )

    bitcode_linkables = []
    for link_item in opts.links + opts.binary_links:
        if link_item.infos == None:
            continue
        for link_info in link_item.infos:
            for linkable in link_info.linkables:
                if isinstance(linkable, ArchiveLinkable) or isinstance(linkable, ObjectsLinkable):
                    if linkable.bitcode_bundle != None:
                        bitcode_linkables.append(linkable.bitcode_bundle)

    if len(bitcode_linkables) > 0:
        bitcode_artifact = make_bitcode_bundle(ctx, output.short_path + ".bc", bitcode_linkables)
    else:
        bitcode_artifact = None

    # Pass to the link wrapper the paths to the .dwo/.o files to rewrite, if we are
    # using split debug with content-based paths.
    if (
        cxx_toolchain_info.split_debug_mode != SplitDebugMode("none")
        and cxx_toolchain_info.cxx_compiler_info.supports_content_based_paths
        and
        # Darwin does not embed paths in object files themselves, but rather
        # the linker writes those paths based on the location of object files passed
        # to the link.
        linker_info.type != LinkerType("darwin")
    ):
        links_to_rewrite = make_artifact_tset(
            ctx.actions,
            ctx.label,
            children = [unpack_external_debug_info(ctx.actions, link) for link in opts.links],
        )
        separate_debug_info_path_file, _ = ctx.actions.write(
            output.short_path + ".split_debug_paths",
            project_artifacts(ctx.actions, links_to_rewrite),
            allow_args = True,
            has_content_based_path = False,
        )
        separate_debug_info_args = cmd_args(
            "--rewrite-content-based-dwo-paths",
            separate_debug_info_path_file,
            "--content-based-dwo-suffix",
            ".dwo" if cxx_toolchain_info.split_debug_mode == SplitDebugMode("split") else ".o",
        )
    else:
        separate_debug_info_args = []

    command = cmd_args(
        link_cmd_parts.linker,
        separate_debug_info_args,
        cmd_args(argfile, format = "@{}"),
        hidden = [
            link_unit_generation_link_args.link_args,
            link_unit_generation_link_args.hidden,
        ]
        + ([ilk_artifact.as_output()] if ilk_artifact else []),
    )

    category = "cxx_link"
    if opts.category_suffix != None:
        category += "_" + opts.category_suffix

    # If the linked object files don't contain debug info, clang may not
    # generate a DWO directory, so make sure we at least `mkdir` and empty
    # one to make v2/RE happy.
    if split_debug_output != None:
        command = cmd_args(
            "/bin/sh",
            "-c",
            cmd_args(split_debug_output.as_output(), format = 'mkdir -p {}; "$@"'),
            '""',
            command,
        )

    if opts.stderr_output != None:
        command = cmd_args(
            cxx_toolchain_info.internal_tools.stderr_to_file,
            cmd_args(opts.stderr_output, format = "--out={}"),
            command,
        )

    if shared_library_interface != None and not linker_emits_shared_library_interface:
        command = cmd_args(
            cxx_toolchain_info.internal_tools.shlib_interface_link_wrapper,
            output.as_output(),
            shared_library_interface.as_output(),
            shared_library_interface_producer(linker_info),
            "--shlib-interface-link-wrapper-link-command",
            command,
        )

    link_execution_preference_info = LinkExecutionPreferenceInfo(
        preference = opts.link_execution_preference,
    )
    action_execution_properties = get_action_execution_attributes(
        opts.link_execution_preference,
    )
    if is_incremental_link:
        action_execution_properties = ActionExecutionAttributes(
            prefer_local = False,
            prefer_remote = False,
            local_only = True,
            full_hybrid = action_execution_properties.full_hybrid,
        )

    enable_late_build_info_stamping = is_result_executable and cxx_stamp_build_info(ctx, link_groups_enabled = link_groups_enabled)

    if is_incremental_link:
        allow_cache_upload = False
    elif enable_late_build_info_stamping:
        allow_cache_upload = True
    else:
        # Preserves `None`: no preference, as opposed to a decision not to upload.
        allow_cache_upload = opts.allow_cache_upload

    ctx.actions.run(
        command,
        prefer_local = action_execution_properties.prefer_local,
        prefer_remote = action_execution_properties.prefer_remote,
        local_only = action_execution_properties.local_only,
        weight = opts.link_weight,
        category = category,
        identifier = opts.identifier,
        force_full_hybrid_if_capable = action_execution_properties.full_hybrid,
        allow_cache_upload = allow_cache_upload,
        error_handler = opts.error_handler,
        no_outputs_cleanup = is_incremental_link,
    )

    external_debug_info = link_external_debug_info(
        ctx = ctx,
        links = opts.links + opts.binary_links,
        split_debug_output = split_debug_output,
        pdb = link_unit_generation_link_args.pdb_artifact,
    )

    unstripped_output = output
    if opts.strip:
        strip_args = opts.strip_args_factory(ctx) if opts.strip_args_factory else cmd_args()
        output = strip_object(ctx, cxx_toolchain_info, output, strip_args, opts.category_suffix, allow_cache_upload = enable_late_build_info_stamping)

    prebolt_output = output
    use_bolt = is_result_executable and cxx_use_bolt(ctx)
    if use_bolt:
        bolt_output = bolt(ctx, output, external_debug_info, opts.identifier, dwp_tool_available, allow_cache_upload = enable_late_build_info_stamping)
        output = bolt_output.output
        split_debug_output = bolt_output.dwo_output

    dwp_artifact = None
    if dwp_tool_available:
        dwp_inputs = cmd_args()
        if use_bolt:
            dwp_inputs.add([split_debug_output])
        else:
            for link in opts.links + opts.binary_links:
                dwp_inputs.add(unpack_link_args(link))
            dwp_inputs.add(project_artifacts(ctx.actions, external_debug_info))

        dwp_artifact = dwp(
            ctx,
            cxx_toolchain_info,
            output,
            identifier = opts.identifier,
            category_suffix = opts.category_suffix,
            # TODO(T110378142): Ideally, referenced objects are a list of
            # artifacts, but currently we don't track them properly.  So, we
            # just pass in the full link line and extract all inputs from that,
            # which is a bit of an overspecification.
            referenced_objects = [dwp_inputs],
            action_execution_properties = get_action_execution_attributes(
                get_dwp_execution_preference(opts),
            ),
        )

    # Per-TU device-debug stripping runs at compile time (compile.bzl).
    # Rename the linker's -pre_extract output back to the canonical name.
    if hip_debug_extract_available(cxx_toolchain_info) and output.short_path.endswith(PRE_EXTRACT_SUFFIX):
        renamed = ctx.actions.declare_output(
            output.short_path.removesuffix(PRE_EXTRACT_SUFFIX),
            has_content_based_path = False,
        )
        ctx.actions.copy_file(renamed.as_output(), output)
        output = renamed

    if is_result_executable:
        output = add_elf_sections_to_executable(
            ctx,
            output,
            action_execution_properties = action_execution_properties,
            has_content_based_path = output_has_content_based_path,
        )
        output = stamp_build_info(
            ctx,
            output,
            links = opts.links,
            build_info_json = build_info_json,
            link_groups_enabled = link_groups_enabled,
        )

    linked_object = LinkedObject(
        output = output,
        link_args = opts.links + opts.binary_links,
        bitcode_bundle = bitcode_artifact.artifact if bitcode_artifact else None,
        prebolt_output = prebolt_output,
        unstripped_output = unstripped_output,
        dwp = dwp_artifact,
        external_debug_info = external_debug_info,
        linker_argsfile = argfile,
        linker_command = command,
        import_library = opts.import_library,
        pdb = link_unit_generation_link_args.pdb_artifact,
        ilk = ilk_artifact,
        split_debug_output = split_debug_output,
    )

    return CxxLinkResult(
        linked_object = linked_object,
        linker_map_data = linker_map_data,
        gc_sections_data = gc_sections_data,
        link_execution_preference_info = link_execution_preference_info,
        sanitizer_runtime_files = sanitizer_runtime_args.sanitizer_runtime_files,
        extra_outputs = extra_linker_outputs.providers,
        shared_library_interface = shared_library_interface,
    )

_AnonLinkInfo = provider(
    fields = {
        "result": provider_field(typing.Any, default = None),  # CxxLinkResult
    }
)

# dwp and split_debug_output are optional outputs, but promise artifacts require an actual artifact
# when being resolved. Let's add some placeholders here so that we always generate an artifact when
# applying the map functions.
_AnonLinkInfoPlaceholder = provider(
    fields = {
        "dwp": provider_field(typing.Any),
        "split_debug_output": provider_field(typing.Any),
    }
)

def _anon_link_impl(ctx):
    (output, result_type, opts) = deserialize_anon_attrs(ctx.actions, ctx.label, ctx.attrs)

    link_result = _cxx_link(
        ctx = ctx,
        output = output,
        result_type = result_type,
        opts = opts,
    )

    dwp_placeholder = ctx.actions.write("placeholder_dwp", "", has_content_based_path = False)
    split_debug_output_placeholder = ctx.actions.write("placeholder_split_debug_output", "", has_content_based_path = False)

    return [
        EMPTY_DEFAULT_INFO,
        _AnonLinkInfo(result = link_result),
        _AnonLinkInfoPlaceholder(dwp = dwp_placeholder, split_debug_output = split_debug_output_placeholder),
    ]

_anon_link_rule = anon_rule(
    impl = _anon_link_impl,
    attrs = ANON_ATTRS,
    artifact_promise_mappings = {
        "dwp": lambda p: _get_link_artifact(p, "dwp"),
        "output": lambda p: p[_AnonLinkInfo].result.linked_object.output,
        "split_debug_output": lambda p: _get_link_artifact(p, "split_debug_output"),
    },
)

def _get_link_artifact(p: ProviderCollection, name: str) -> Artifact:
    linked_object = p[_AnonLinkInfo].result.linked_object
    if getattr(linked_object, name) != None:
        return getattr(linked_object, name)
    else:
        return getattr(p[_AnonLinkInfoPlaceholder], name)

def _anon_cxx_link(
    ctx: AnalysisContext, output: str, result_type: CxxLinkResultType, opts: LinkOptions, script_flags: list[(Artifact, str)] = []
) -> CxxLinkResult:
    if opts.stderr_output != None:
        fail("stderr_output is not supported for anonymous link actions")
    if opts.cxx_toolchain:
        fail("anon link requires getting toolchain from ctx.attrs._cxx_toolchain")
    if opts.produce_shared_library_interface:
        fail("anon link cannot produce a shared library interface")
    cxx_toolchain = ctx.attrs._cxx_toolchain[CxxToolchainInfo]
    anon_link_target = ctx.actions.anon_target(
        _anon_link_rule,
        dict(
            _cxx_toolchain = ctx.attrs._cxx_toolchain,
            **serialize_anon_attrs(
                output = output,
                result_type = result_type,
                opts = opts,
                script_flags = script_flags,
            ),
        ),
    )

    dwp = None
    if dwp_available(cxx_toolchain):
        dwp = anon_link_target.artifact("dwp")

    split_debug_output = None
    if generates_split_debug(cxx_toolchain):
        split_debug_output = anon_link_target.artifact("split_debug_output")

    output = ctx.actions.assert_short_path(anon_link_target.artifact("output"), short_path = output)

    external_debug_info = link_external_debug_info(
        ctx = ctx,
        links = opts.links,
        split_debug_output = split_debug_output,
    )

    # The anon target API doesn't allow us to return the list of artifacts for
    # sanitizer runtime, so it has be computed here
    sanitizer_runtime_args = cxx_sanitizer_runtime_arguments(ctx, cxx_toolchain, output)

    return CxxLinkResult(
        linked_object = LinkedObject(
            output = output,
            unstripped_output = output,
            dwp = dwp,
            external_debug_info = external_debug_info,
        ),
        linker_map_data = None,
        gc_sections_data = None,
        link_execution_preference_info = LinkExecutionPreferenceInfo(
            preference = LinkExecutionPreference("any"),
        ),
        sanitizer_runtime_files = sanitizer_runtime_args.sanitizer_runtime_files,
        shared_library_interface = None,
    )

def _cxx_link(
    ctx: AnalysisContext, output: str, result_type: CxxLinkResultType, opts: LinkOptions, anonymous: bool = False, anon_script_flags: list[(Artifact, str)] = []
):
    if anonymous:
        return _anon_cxx_link(
            ctx = ctx,
            output = output,
            result_type = result_type,
            opts = opts,
            script_flags = anon_script_flags,
        )
    if anon_script_flags:
        fail("anon_script_flags is only consumed by anonymous links; pass these flags in `opts` instead")
    return cxx_link_into(
        ctx = ctx,
        output = ctx.actions.declare_output(output, has_content_based_path = False),
        result_type = result_type,
        opts = opts,
    )

def cxx_link_shared_library(
    ctx: AnalysisContext,
    # The destination for the link output.
    output: str,
    opts: LinkOptions,
    # Optional soname to link into shared library.
    name: [str, None] = None,
    # Overrides the default flags used to specify building shared libraries
    shared_library_flags: [SharedLibraryFlagOverrides, None] = None,
    anonymous: bool = False,
    # (artifact, format) linker flags for an anonymous link. See
    # `serialize_anon_attrs`; non-anonymous links take these in `opts`.
    anon_script_flags: list[(Artifact, str)] = [],
) -> CxxLinkResult:
    # links: list[LinkArgs] = [],
    # link_execution_preference: LinkExecutionPreference = LinkExecutionPreference("any"),

    """
    Link a shared library into the supplied output.
    """
    merged_opts = _build_cxx_link_shared_library_options(
        ctx = ctx,
        output = output,
        opts = opts,
        name = name,
        shared_library_flags = shared_library_flags,
    )

    return _cxx_link(
        ctx = ctx,
        output = output,
        result_type = CxxLinkResultType("shared_library"),
        opts = merged_opts,
        anonymous = anonymous,
        anon_script_flags = anon_script_flags,
    )

def _build_cxx_link_shared_library_options(
    ctx: AnalysisContext,
    # The destination for the link output.
    output: str,
    opts: LinkOptions,
    # Optional soname to link into shared library.
    name: [str, None] = None,
    # Overrides the default flags used to specify building shared libraries
    shared_library_flags: [SharedLibraryFlagOverrides, None] = None,
) -> LinkOptions:
    cxx_toolchain = opts.cxx_toolchain or get_cxx_toolchain_info(ctx)
    linker_info = cxx_toolchain.linker_info
    linker_type = linker_info.type
    extra_args = []

    extra_args.extend(get_shared_library_flags(linker_type, shared_library_flags))  # e.g. "-shared"
    if name != None:
        extra_args.extend(get_shared_library_name_linker_flags(linker_type, name, shared_library_flags))

    link_execution_preference = opts.link_execution_preference
    if linker_info.link_libraries_locally and link_execution_preference == LinkExecutionPreference("any"):
        link_execution_preference = LinkExecutionPreference("local")

    (import_library, import_library_args) = get_import_library(
        ctx,
        linker_type,
        output,
    )

    deffile_args = get_deffile_flags(ctx, linker_type)

    links_with_extra_args = [LinkArgs(flags = extra_args)] + opts.links + [LinkArgs(flags = import_library_args + deffile_args)]

    return merge_link_options(
        opts,
        links = links_with_extra_args,
        link_execution_preference = link_execution_preference,
        import_library = import_library,
    )
