# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//apple:apple_provisioning_profile_sources.bzl", "AppleProvisioningProfileSourcesInfo")
load("@prelude//apple/swift:swift_toolchain_types.bzl", "SwiftToolchainInfo")
load("@prelude//utils:utils.bzl", "value_or")
load(":apple_bundle_destination.bzl", "AppleBundleDestination", "bundle_relative_path_for_destination")
load(
    ":apple_bundle_types.bzl",
    "AppleBundleCodesignManifestTree",
    "AppleBundleManifest",
    "AppleBundleManifestInfo",
    "AppleBundleManifestLogFiles",
    "AppleBundleSigningContextTree",
)
load(":apple_bundle_utility.bzl", "get_extension_attr", "get_product_name")
load(":apple_code_signing_types.bzl", "CodeSignConfiguration", "CodeSignType", "get_code_signing_configuration_attr_value")
load(":apple_entitlements.bzl", "get_entitlements_codesign_args", "should_include_entitlements")
load(":apple_error_handler.bzl", "apple_build_error_handler")
load(":apple_sdk.bzl", "get_apple_sdk_name")
load(":apple_sdk_metadata.bzl", "get_apple_sdk_metadata_for_sdk_name")
load(":apple_swift_stdlib.bzl", "should_copy_swift_stdlib")
load(":apple_toolchain_types.bzl", "AppleToolchainInfo", "AppleToolsInfo")

# Must match the null case returned by `serialize_signing_context_data`:
# https://www.internalfb.com/code/fbsource/fbcode/buck2/prelude/apple/tools/bundling/signing_context_data.py?lines=39%2C58
_EMPTY_SIGNING_CONTEXT_DATA = {
    "provisioning_profile_data_base64": None,
    "selected_identity": None,
    "signing_context": None,
    "version": 1,
}

# Defines where and what should be copied into
AppleBundlePart = record(
    # A file or directory which content should be copied
    source = field(Artifact),
    # Where the source should be copied, the actual destination directory
    # inside bundle depends on target platform
    destination = AppleBundleDestination,
    # New file name if it should be renamed before copying.
    # Empty string value is applicable only when `source` is a directory,
    # in such case only content of the directory will be copied, as opposed to the directory itself.
    # When value is `None`, directory or file will be copied as it is, without renaming.
    new_name = field([str, None], None),
    # Marks parts which should be code signed separately from the whole bundle.
    codesign_on_copy = field(bool, False),
    # Entitlements to use when this part is code signed separately.
    codesign_entitlements = field(Artifact | None, None),
    # If present, override the codesign flags with these flags, when this part is code signed separately.
    codesign_flags_override = field([list[str], None], None),
    # If present, additional paths to be codesigned when copying.
    extra_codesign_paths = field([list[str], None], None),
)

# Represents an `AppleBundlePart` which has an associated codesign manifest
AppleBundleCodesignManifestTreePart = record(
    bundle_part = field(AppleBundlePart),
    codesign_manifest_tree = field(AppleBundleCodesignManifestTree),
)

# Represents an `AppleBundlePart` which has an associated signing context
AppleBundleSigningContextTreePart = record(
    bundle_part = field(AppleBundlePart),
    signing_context_tree = field(AppleBundleSigningContextTree),
)

SwiftStdlibArguments = record(
    primary_binary_rel_path = field(str),
)

AppleBundleConstructionResult = record(
    providers = field(list[Provider]),
    sub_targets = field(dict[str, list[Provider]]),
    codesign_manifest_tree = field(AppleBundleCodesignManifestTree),
    signing_context_tree = field(AppleBundleSigningContextTree),
    signing_info = field(Artifact),
)

def bundle_output(ctx: AnalysisContext) -> Artifact:
    bundle_dir_name = get_bundle_dir_name(ctx)
    output = ctx.actions.declare_output(bundle_dir_name, has_content_based_path = False)
    return output

def _bundling_log_args(ctx: AnalysisContext, output_name: str):
    if not ctx.attrs._bundling_log_file_enabled:
        return ([], None)

    output = ctx.actions.declare_output(output_name, has_content_based_path = False)
    args = ["--log-file", output.as_output()]
    if ctx.attrs._bundling_log_file_level:
        args.extend(["--log-level-file", ctx.attrs._bundling_log_file_level])
    return (args, output)

def assemble_bundle(
    ctx: AnalysisContext,
    bundle: Artifact,
    parts: list[AppleBundlePart],
    codesign_manifest_parts: list[AppleBundleCodesignManifestTreePart],
    signing_context_parts: list[AppleBundleSigningContextTreePart],
    info_plist_part: [AppleBundlePart, None],
    swift_stdlib_args: [SwiftStdlibArguments, None],
    extra_hidden: list[Artifact] = [],
    skip_adhoc_signing: bool = False,
    incremental_bundling_override = None,
) -> AppleBundleConstructionResult:
    """
    Returns extra subtargets related to bundling.
    """
    all_parts = parts + [info_plist_part] if info_plist_part else []
    codesign_type = _detect_codesign_type(ctx, skip_adhoc_signing)
    spec_file = _bundle_spec_json(ctx, all_parts, codesign_type)

    tools = ctx.attrs._apple_tools[AppleToolsInfo]
    tool = tools.assemble_bundle

    codesign_selection_args = []
    codesign_execution_args = []
    codesign_bundle_extra_args = []

    codesign_tool = ctx.attrs._apple_toolchain[AppleToolchainInfo].codesign
    if ctx.attrs._codesign_command_override:
        codesign_tool = ctx.attrs._codesign_command_override[RunInfo]

    code_signing_configuration = get_code_signing_configuration_attr_value(ctx)
    if code_signing_configuration == CodeSignConfiguration("dry-run"):
        codesign_configuration_args = ["--codesign-configuration", "dry-run"]
        codesign_tool = tools.dry_codesign_tool
    elif code_signing_configuration == CodeSignConfiguration("execution-bypass"):
        codesign_configuration_args = ["--codesign-configuration", "execution-bypass"]
    elif code_signing_configuration == CodeSignConfiguration("fast-adhoc"):
        if _get_fast_adhoc_signing_enabled(ctx):
            codesign_configuration_args = ["--codesign-configuration", "fast-adhoc"]
            if getattr(ctx.attrs, "_fast_adhoc_signing_probe_enabled", False):
                codesign_configuration_args = codesign_configuration_args + ["--fast-adhoc-signing-probe-enabled"]
        else:
            codesign_configuration_args = []
    elif code_signing_configuration == CodeSignConfiguration("none"):
        codesign_configuration_args = []
    else:
        fail("Code signing configuration `{}` not supported".format(code_signing_configuration))

    codesign_required = codesign_type != CodeSignType("skip")
    swift_support_required = swift_stdlib_args and (not ctx.attrs.skip_copying_swift_stdlib) and should_copy_swift_stdlib(bundle.extension)

    sdk_name = get_apple_sdk_name(ctx)
    if codesign_required or swift_support_required:
        platform_args = ["--platform", sdk_name]
    else:
        platform_args = []

    if swift_support_required:
        swift_args = [
            "--binary-destination",
            swift_stdlib_args.primary_binary_rel_path,
            "--frameworks-destination",
            bundle_relative_path_for_destination(AppleBundleDestination("frameworks"), sdk_name, ctx.attrs.extension, ctx.attrs.versioned_macos_bundle),
            "--plugins-destination",
            bundle_relative_path_for_destination(AppleBundleDestination("plugins"), sdk_name, ctx.attrs.extension, ctx.attrs.versioned_macos_bundle),
            "--extensionkit-extensions-destination",
            bundle_relative_path_for_destination(
                AppleBundleDestination("extensionkit_extensions"), sdk_name, ctx.attrs.extension, ctx.attrs.versioned_macos_bundle
            ),
            "--appclips-destination",
            bundle_relative_path_for_destination(AppleBundleDestination("appclips"), sdk_name, ctx.attrs.extension, ctx.attrs.versioned_macos_bundle),
            "--swift-stdlib-command",
            cmd_args(ctx.attrs._apple_toolchain[SwiftToolchainInfo].swift_stdlib_tool, delimiter = " ", quote = "shell"),
            "--sdk-root",
            ctx.attrs._apple_toolchain[SwiftToolchainInfo].sdk_path,
        ]
    else:
        swift_args = []

    if codesign_required:
        codesign_execution_args += [
            "--codesign",
        ]

        codesign_bundle_extra_args += [
            "--codesign-tool",
            codesign_tool,
        ]

        profile_selection_required = _should_embed_provisioning_profile(ctx, codesign_type)
        if profile_selection_required:
            sources_info = ctx.attrs._provisioning_profile_sources[AppleProvisioningProfileSourcesInfo]
            for i, source in enumerate(sources_info.sources):
                if source.profiles:
                    profiles_dir = ctx.actions.symlinked_dir(
                        "provisioning_profile_source_{}".format(i),
                        {profile.short_path: profile for profile in source.profiles},
                        has_content_based_path = False,
                    )
                    codesign_selection_args.extend(["--profiles-dir", profiles_dir])
                elif source.directory:
                    codesign_selection_args.extend(["--profiles-dir", source.directory])

            identities_command = ctx.attrs._apple_toolchain[AppleToolchainInfo].codesign_identities_command
            if ctx.attrs._codesign_identities_command_override:
                identities_command = ctx.attrs._codesign_identities_command_override[RunInfo]
            identities_command_args = ["--codesign-identities-command", cmd_args(identities_command)] if identities_command else []
            codesign_selection_args.extend(identities_command_args)

        if codesign_type == CodeSignType("adhoc"):
            codesign_selection_args.append("--ad-hoc")
            if ctx.attrs.codesign_identity:
                codesign_selection_args.extend(["--ad-hoc-codesign-identity", ctx.attrs.codesign_identity])
            if profile_selection_required:
                codesign_selection_args.append("--embed-provisioning-profile-when-signing-ad-hoc")

        entitlements_args = get_entitlements_codesign_args(ctx, codesign_type)
        codesign_execution_args += entitlements_args

        if getattr(ctx.attrs, "entitlements_suffixed_key_map", None):
            arg = ["--entitlements-suffixed-key-map", json.encode(ctx.attrs.entitlements_suffixed_key_map)]
            codesign_execution_args += arg
        if getattr(ctx.attrs, "entitlements_removed_keys", None):
            arg = ["--entitlements-removed-keys", json.encode(ctx.attrs.entitlements_removed_keys)]
            codesign_execution_args += arg
        if getattr(ctx.attrs, "entitlements_removed_values_map", None):
            arg = ["--entitlements-removed-values-map", json.encode(ctx.attrs.entitlements_removed_values_map)]
            codesign_execution_args += arg
        codesign_bundle_extra_args += _get_extra_codesign_args(ctx)

        info_plist_args = (
            [
                "--info-plist-source",
                info_plist_part.source,
                "--info-plist-destination",
                get_apple_bundle_part_relative_destination_path(ctx, info_plist_part),
            ]
            if info_plist_part
            else []
        )
        codesign_execution_args.extend(info_plist_args)

        if ctx.attrs.provisioning_profile_filter:
            arg = [
                "--provisioning-profile-filter",
                ctx.attrs.provisioning_profile_filter,
            ]
            codesign_selection_args.extend(arg)

        strict_provisioning_profile_search = value_or(ctx.attrs.strict_provisioning_profile_search, ctx.attrs._strict_provisioning_profile_search_default)
        if strict_provisioning_profile_search:
            codesign_selection_args.append("--strict-provisioning-profile-search")

        if ctx.attrs._fast_provisioning_profile_parsing_enabled:
            codesign_selection_args.append("--fast-provisioning-profile-parsing")

        if ctx.attrs._no_check_certificates:
            codesign_selection_args.append("--no-check-certificates")

        if ctx.attrs.entitlements_verification_check_enabled:
            codesign_selection_args.append("--verify-entitlements")

    elif codesign_type == CodeSignType("skip"):
        pass
    else:
        fail("Code sign type `{}` not supported".format(codesign_type))

    force_local_bundling = codesign_required
    env = {}
    cache_buster = ctx.attrs._bundling_cache_buster
    if cache_buster:
        env["BUCK2_BUNDLING_CACHE_BUSTER"] = cache_buster

    if codesign_required:
        signing_context_path = ctx.actions.declare_output("signing_context.json", has_content_based_path = False)
        resolve_command = [
            tools.resolve_signing_context,
            "--output",
            signing_context_path.as_output(),
        ]
        resolution_log_args, resolution_log_output = _bundling_log_args(ctx, "signing_context_resolution_log.txt")
        resolve_command.extend(resolution_log_args)
        ctx.actions.run(
            cmd_args(
                resolve_command + platform_args + codesign_selection_args + codesign_execution_args,
            ),
            local_only = force_local_bundling,
            prefer_local = not force_local_bundling,
            category = "apple_resolve_signing_context",
            env = env,
            error_handler = apple_build_error_handler,
        )
        signing_context_path_arg = ["--signing-context-path", signing_context_path]

        signing_info_output = ctx.actions.declare_output("signing-info.json", has_content_based_path = False)
        ctx.actions.run(
            cmd_args(
                [
                    tools.signing_info,
                    "--signing-context-path",
                    signing_context_path,
                    "--output",
                    signing_info_output.as_output(),
                ],
            ),
            category = "apple_signing_info",
            error_handler = apple_build_error_handler,
        )
    else:
        resolution_log_output = None
        # Avoid adding a resolver action to every unsigned bundle target.
        signing_context_path = ctx.actions.write_json(
            "signing_context.json",
            _EMPTY_SIGNING_CONTEXT_DATA,
        )
        signing_context_path_arg = ["--signing-context-path", signing_context_path]
        signing_info_output = ctx.actions.write_json("signing-info.json", {})

    # - Always request codesign manifest, even if signing not required.
    #   Removes the need for conditional subtargets and fields in JSON output.
    #  - Manifest file name reflects whether signing is required or not.
    #    Useful for debugging purposes.
    codesign_manifest_file_name = "codesign_manifest.json" if codesign_required else "placeholder_codesign_manifest.json"
    codesign_manifest = ctx.actions.declare_output(codesign_manifest_file_name, has_content_based_path = False)
    codesign_bundle_extra_args += [
        "--codesign-manifest",
        codesign_manifest.as_output(),
    ]

    extra_context_args = [
        "--resources-destination",
        bundle_relative_path_for_destination(AppleBundleDestination("resources"), sdk_name, ctx.attrs.extension, ctx.attrs.versioned_macos_bundle),
    ]

    command = cmd_args(
        [
            tool,
            "--output",
            bundle.as_output(),
            "--spec",
            spec_file,
        ]
        + codesign_execution_args
        + codesign_bundle_extra_args
        + platform_args
        + swift_args
        + extra_context_args
        + signing_context_path_arg,
        hidden = [part.source for part in all_parts]
        + [part.codesign_entitlements for part in all_parts if part.codesign_entitlements]
        +
        # Ensures any genrule deps get built, such targets are used for validation
        extra_hidden,
    )
    run_incremental_args = {}
    incremental_state = ctx.actions.declare_output("incremental_state.json", has_content_based_path = False).as_output()

    # Fallback to value from buckconfig
    incremental_bundling_enabled = ctx.attrs.incremental_bundling_enabled or ctx.attrs._incremental_bundling_enabled
    if incremental_bundling_override != None:
        incremental_bundling_enabled = incremental_bundling_override

    if incremental_bundling_enabled:
        command.add("--incremental-state", incremental_state)
        run_incremental_args = {
            "metadata_env_var": "ACTION_METADATA",
            "metadata_path": "action_metadata.json",
            "no_outputs_cleanup": True,
        }
        category = "apple_assemble_bundle_incremental"
    else:
        # overwrite file with incremental state so if previous and next builds are incremental
        # (as opposed to the current non-incremental one), next one won't assume there is a
        # valid incremental state.
        command.add(cmd_args(hidden = ctx.actions.write_json(incremental_state, {})))
        category = "apple_assemble_bundle"

    if ctx.attrs._profile_bundling_enabled:
        profile_output = ctx.actions.declare_output("bundling_profile.txt", has_content_based_path = False).as_output()
        command.add("--profile-output", profile_output)

    subtargets = {}
    if resolution_log_output:
        subtargets["signing-context-resolution-log"] = [DefaultInfo(default_output = resolution_log_output)]
    bundling_log_args, bundling_log_output = _bundling_log_args(ctx, "bundling_log.txt")
    command.add(bundling_log_args)
    if bundling_log_output:
        subtargets["bundling-log"] = [DefaultInfo(default_output = bundling_log_output)]

    command.add("--check-conflicts")
    if ctx.attrs.versioned_macos_bundle:
        command.add("--versioned-if-macos")
    if getattr(ctx.attrs, "include_build_info_file", False):
        extension_allowed = ctx.attrs.extension in ctx.attrs.include_build_info_file_in_bundles_with_extensions
        if extension_allowed:
            command.add("--include-build-info-file")
            command.add("--build-info-swift-dynamic-replacement", "true" if getattr(ctx.attrs, "_swift_dynamic_replacement_enabled", False) else "false")
    command.add(codesign_configuration_args)

    bundle_telemetry_logger = tools.bundle_telemetry_logger
    if bundle_telemetry_logger:
        command.add("--bundle-telemetry-logger", bundle_telemetry_logger)

    ctx.actions.run(
        command,
        local_only = force_local_bundling,
        prefer_local = not force_local_bundling,
        category = category,
        env = env,
        error_handler = apple_build_error_handler,
        **run_incremental_args,
    )

    command_json = ctx.actions.declare_output("bundling_command.json", has_content_based_path = False)
    command_json_cmd_args = ctx.actions.write_json(command_json, command, with_inputs = True, pretty = True)
    subtargets["command"] = [DefaultInfo(default_output = command_json, other_outputs = [command_json_cmd_args])]

    bundle_manifest_log_file_map = {
        ctx.label: AppleBundleManifestLogFiles(
            command_file = command_json,
            spec_file = spec_file,
            log_file = bundling_log_output,
        ),
    }

    if hasattr(ctx.attrs, "deps"):
        for dep in ctx.attrs.deps:
            dep_manifest_info = dep.get(AppleBundleManifestInfo)
            if dep_manifest_info:
                bundle_manifest_log_file_map.update(dep_manifest_info.manifest.log_file_map)

    bundle_manifest = AppleBundleManifest(log_file_map = bundle_manifest_log_file_map)
    bundle_manifest_json_object = _convert_bundle_manifest_to_json_object(bundle_manifest)

    bundle_manifest_json_file = ctx.actions.declare_output("bundle_manifest.json", has_content_based_path = False)
    bundle_manifest_cmd_args = ctx.actions.write_json(bundle_manifest_json_file, bundle_manifest_json_object, with_inputs = True, pretty = True)
    subtargets["manifest"] = [DefaultInfo(default_output = bundle_manifest_json_file, other_outputs = [bundle_manifest_cmd_args])]

    providers = [AppleBundleManifestInfo(manifest = bundle_manifest)]

    codesign_manifest_tree = _make_codesign_manifest_tree(ctx, codesign_manifest, codesign_manifest_parts)
    codesign_manifest_tree_json = _get_codesign_manifest_tree_as_json(codesign_manifest_tree)
    codesign_manifest_tree_json_file = ctx.actions.declare_output("codesign_manifest_tree.json", has_content_based_path = False)
    codesign_manifest_tree_json_cmd_args = ctx.actions.write_json(
        codesign_manifest_tree_json_file,
        codesign_manifest_tree_json,
        with_inputs = True,
        pretty = True,
    )

    tools = ctx.attrs._apple_tools[AppleToolsInfo]
    manifest_tree_postprocessor = tools.codesign_manifest_tree_postprocessor

    postprocessed_codesign_manifest_tree = ctx.actions.declare_output("postprocessed_codesign_manifest_tree.json", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            [
                manifest_tree_postprocessor,
                "--codesign-manifest-tree",
                codesign_manifest_tree_json_cmd_args,
                "--output",
                postprocessed_codesign_manifest_tree.as_output(),
            ],
        ),
        category = "codesign_manifest_tree_postprocess",
    )

    subtargets["codesign-manifest"] = [
        DefaultInfo(
            default_output = postprocessed_codesign_manifest_tree,
        ),
    ]

    signing_context_output = ctx.actions.declare_output("provisioning-manifest.json", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            [
                tools.provisioning_manifest,
                "--output",
                signing_context_output.as_output(),
            ]
            + signing_context_path_arg,
        ),
        category = "apple_provisioning_manifest",
        error_handler = apple_build_error_handler,
    )

    signing_context_tree = _make_signing_context_tree(ctx, signing_context_output, signing_context_parts)
    signing_context_tree_json = _get_signing_context_tree_as_json(signing_context_tree)
    signing_context_tree_json_file = ctx.actions.declare_output("provisioning-manifest-tree.json", has_content_based_path = False)
    signing_context_tree_json_cmd_args = ctx.actions.write_json(
        signing_context_tree_json_file,
        signing_context_tree_json,
        with_inputs = True,
        pretty = True,
    )

    postprocessed_signing_context_tree = ctx.actions.declare_output("postprocessed-provisioning-manifest-tree.json", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            [
                tools.signing_context_tree_postprocessor,
                "--signing-context-tree",
                signing_context_tree_json_cmd_args,
                "--output",
                postprocessed_signing_context_tree.as_output(),
            ],
        ),
        category = "apple_provisioning_manifest_tree_postprocess",
    )

    subtargets["provisioning-manifest"] = [DefaultInfo(default_output = postprocessed_signing_context_tree)]
    subtargets["signing-info"] = [DefaultInfo(default_output = signing_info_output)]

    return AppleBundleConstructionResult(
        sub_targets = subtargets,
        providers = providers,
        codesign_manifest_tree = codesign_manifest_tree,
        signing_context_tree = signing_context_tree,
        signing_info = signing_info_output,
    )

def _make_codesign_manifest_tree(
    ctx: AnalysisContext, codesign_manifest: Artifact, inner_parts: list[AppleBundleCodesignManifestTreePart]
) -> AppleBundleCodesignManifestTree:
    inner_manifest_trees = {}

    for codesign_manifest_tree_part in inner_parts:
        relative_path = get_apple_bundle_part_relative_destination_path(ctx, codesign_manifest_tree_part.bundle_part)
        inner_manifest_trees[relative_path] = codesign_manifest_tree_part.codesign_manifest_tree

    return AppleBundleCodesignManifestTree(
        codesign_manifest = codesign_manifest,
        inner_codesign_manifests = inner_manifest_trees,
    )

def _get_codesign_manifest_tree_as_json(codesign_manifest_tree: AppleBundleCodesignManifestTree) -> dict[str, typing.Any]:
    json_obj = {
        "codesign_manifest": codesign_manifest_tree.codesign_manifest,
    }

    inner_manifests = {}
    for relative_path, inner_codesign_manifest_tree in codesign_manifest_tree.inner_codesign_manifests.items():
        inner_manifests[relative_path] = _get_codesign_manifest_tree_as_json(inner_codesign_manifest_tree)

    json_obj["inner_codesign_manifests"] = inner_manifests
    return json_obj

def _make_signing_context_tree(
    ctx: AnalysisContext, signing_context: Artifact, inner_parts: list[AppleBundleSigningContextTreePart]
) -> AppleBundleSigningContextTree:
    inner_context_trees = {}

    for signing_context_tree_part in inner_parts:
        relative_path = get_apple_bundle_part_relative_destination_path(ctx, signing_context_tree_part.bundle_part)
        inner_context_trees[relative_path] = signing_context_tree_part.signing_context_tree

    return AppleBundleSigningContextTree(
        signing_context = signing_context,
        inner_signing_contexts = inner_context_trees,
    )

def _get_signing_context_tree_as_json(signing_context_tree: AppleBundleSigningContextTree) -> dict[str, typing.Any]:
    json_obj = {
        "signing_context": signing_context_tree.signing_context,
    }

    inner_contexts = {}
    for relative_path, inner_signing_context_tree in signing_context_tree.inner_signing_contexts.items():
        inner_contexts[relative_path] = _get_signing_context_tree_as_json(inner_signing_context_tree)

    json_obj["inner_signing_contexts"] = inner_contexts
    return json_obj

def get_bundle_dir_name(ctx: AnalysisContext) -> str:
    return paths.replace_extension(get_product_name(ctx), "." + get_extension_attr(ctx))

def get_apple_bundle_part_relative_destination_path(ctx: AnalysisContext, part: AppleBundlePart) -> str:
    bundle_relative_path = bundle_relative_path_for_destination(
        part.destination, get_apple_sdk_name(ctx), ctx.attrs.extension, ctx.attrs.versioned_macos_bundle
    )
    destination_file_or_directory_name = part.new_name if part.new_name != None else part.source.basename
    return paths.join(bundle_relative_path, destination_file_or_directory_name)

# Returns JSON to be passed into bundle assembling tool. It should contain a dictionary which maps bundle relative destination paths to source paths."
def _bundle_spec_json(ctx: AnalysisContext, parts: list[AppleBundlePart], codesign_type: CodeSignType) -> Artifact:
    specs = []
    include_entitlements = should_include_entitlements(ctx, codesign_type)

    for part in parts:
        part_spec = {
            "dst": get_apple_bundle_part_relative_destination_path(ctx, part),
            "src": part.source,
        }
        if part.codesign_on_copy:
            part_spec["codesign_on_copy"] = True
            if include_entitlements and part.codesign_entitlements:
                part_spec["codesign_entitlements"] = part.codesign_entitlements
            if part.codesign_flags_override:
                part_spec["codesign_flags_override"] = part.codesign_flags_override
            if part.extra_codesign_paths:
                part_spec["extra_codesign_paths"] = part.extra_codesign_paths
        specs.append(part_spec)

    return ctx.actions.write_json("bundle_spec.json", specs, pretty = True, has_content_based_path = False)

def _get_codesign_type_from_attribs(ctx: AnalysisContext) -> [CodeSignType, None]:
    # Target-level attribute takes highest priority
    if ctx.attrs.codesign_type:
        return CodeSignType(ctx.attrs.codesign_type)

    # Config-based global default
    if ctx.attrs._codesign_type:
        return CodeSignType(ctx.attrs._codesign_type)
    return None

def _detect_codesign_type(ctx: AnalysisContext, skip_adhoc_signing: bool) -> CodeSignType:
    def compute_codesign_type():
        if ctx.attrs.extension not in ["app", "appex", "xctest", "driver"]:
            # Only code sign application bundles, extensions and test bundles
            return CodeSignType("skip")

        codesign_type_attrib = _get_codesign_type_from_attribs(ctx)
        if codesign_type_attrib != None:
            return codesign_type_attrib

        sdk_name = get_apple_sdk_name(ctx)
        is_ad_hoc_sufficient = get_apple_sdk_metadata_for_sdk_name(sdk_name).is_ad_hoc_code_sign_sufficient
        return CodeSignType("adhoc" if is_ad_hoc_sufficient else "distribution")

    codesign_type = compute_codesign_type()
    if skip_adhoc_signing and codesign_type == CodeSignType("adhoc"):
        codesign_type = CodeSignType("skip")

    return codesign_type

def _get_extra_codesign_args(ctx: AnalysisContext) -> list[str]:
    codesign_args = ctx.attrs.codesign_flags if hasattr(ctx.attrs, "codesign_flags") else []
    return ["--codesign-args={}".format(flag) for flag in codesign_args]

def _should_embed_provisioning_profile(ctx: AnalysisContext, codesign_type: CodeSignType) -> bool:
    if codesign_type == CodeSignType("distribution"):
        return True

    if codesign_type == CodeSignType("adhoc"):
        # The config-based override value takes priority over target value
        if ctx.attrs._embed_provisioning_profile_when_adhoc_code_signing != None:
            return ctx.attrs._embed_provisioning_profile_when_adhoc_code_signing
        return ctx.attrs.embed_provisioning_profile_when_adhoc_code_signing

    return False

def _convert_bundle_manifest_to_json_object(manifest: AppleBundleManifest) -> dict[Label, typing.Any]:
    manifest_dict = {}
    for target_label, logs in manifest.log_file_map.items():
        manifest_dict[target_label] = {
            "command": logs.command_file,
            "log": logs.log_file,
            "spec": logs.spec_file,
        }
    return manifest_dict

def _get_fast_adhoc_signing_enabled(ctx: AnalysisContext) -> bool:
    fast_adhoc_signing_enabled = ctx.attrs.fast_adhoc_signing_enabled
    if fast_adhoc_signing_enabled != None:
        return fast_adhoc_signing_enabled
    return ctx.attrs._fast_adhoc_signing_enabled_default
