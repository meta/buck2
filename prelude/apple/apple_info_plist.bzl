# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(":apple_bundle_destination.bzl", "AppleBundleDestination")
load(":apple_bundle_part.bzl", "AppleBundlePart")
load(
    ":apple_bundle_utility.bzl",
    "get_bundle_min_target_version",
    "get_default_binary_dep",
    "get_product_name",
    "get_resources_execution_group",
    "get_resources_sdk_name",
    "get_resources_toolchain_info",
)
load(
    ":apple_sdk_metadata.bzl",
    "AppleSdkMetadata",  # @unused Used as a type
    "MacOSXCatalystSdkMetadata",
    "MacOSXSdkMetadata",
    "WatchOSSdkMetadata",
    "WatchSimulatorSdkMetadata",
    "get_apple_sdk_metadata_for_sdk_name",
)
load(":apple_target_sdk_version.bzl", "get_platform_name_for_sdk", "get_platform_version_for_sdk_version")
load(":apple_toolchain_types.bzl", "AppleToolsInfo")

UpdateOperations = enum("set", "insert")
MergeOperations = enum("merge")
RestrictedMergeOperations = enum("copy")

def process_info_plist(ctx: AnalysisContext, override_input: Artifact | None) -> AppleBundlePart:
    input = _preprocess_info_plist(ctx)
    output = ctx.actions.declare_output("Info.plist", has_content_based_path = False)
    additional_keys = _additional_keys_as_json_file(ctx)
    override_keys = _override_keys_as_json_file(ctx)
    process_plist(
        ctx = ctx,
        input = input,
        output = output.as_output(),
        override_input = override_input,
        additional_keys = additional_keys,
        override_keys = override_keys,
    )
    return AppleBundlePart(source = output, destination = AppleBundleDestination("metadata"))

def _get_plist_run_options(ctx: AnalysisContext) -> dict[str, typing.Any]:
    return {
        # Output is deterministic, so can be cached
        "allow_cache_upload": True,
        "exec_group": get_resources_execution_group(ctx),
        # plist generation is cheap and fast, RE network overhead not worth it
        "prefer_local": True,
    }

def _preprocess_info_plist(ctx: AnalysisContext) -> Artifact:
    input = ctx.attrs.info_plist
    output = ctx.actions.declare_output("PreprocessedInfo.plist", has_content_based_path = False)
    substitutions_json = _plist_substitutions_as_json_file(ctx)
    apple_tools = ctx.attrs._apple_tools[AppleToolsInfo]
    processor = apple_tools.info_plist_processor
    command = cmd_args([
        processor,
        "preprocess",
        "--input",
        input,
        "--output",
        output.as_output(),
        "--product-name",
        get_product_name(ctx),
    ])
    if substitutions_json != None:
        command.add(["--substitutions-json", substitutions_json])
    if getattr(ctx.attrs, "enforce_minimum_os_plist_key", False):
        sdk_name = get_resources_sdk_name(ctx)
        sdk_metadata = get_apple_sdk_metadata_for_sdk_name(sdk_name)
        command.add(["--enforce-minimum-os-plist-key", sdk_metadata.min_version_plist_info_key])

    ctx.actions.run(command, category = "apple_preprocess_info_plist", **_get_plist_run_options(ctx))
    return output

def _plist_substitutions_as_json_file(ctx: AnalysisContext) -> Artifact | None:
    info_plist_substitutions = ctx.attrs.info_plist_substitutions
    if not info_plist_substitutions:
        return None

    substitutions_json = ctx.actions.write_json(
        "plist_substitutions.json", info_plist_substitutions, has_content_based_path = False, exec_group = get_resources_execution_group(ctx)
    )
    return substitutions_json

def process_plist(
    ctx: AnalysisContext,
    input: Artifact,
    output: OutputArtifact,
    override_input: Artifact | None = None,
    additional_keys: Artifact | None = None,
    override_keys: Artifact | None = None,
    action_id: [str, None] = None,
):
    apple_tools = ctx.attrs._apple_tools[AppleToolsInfo]
    processor = apple_tools.info_plist_processor
    override_input_arguments = ["--override-input", override_input] if override_input != None else []
    additional_keys_arguments = ["--additional-keys", additional_keys] if additional_keys != None else []
    override_keys_arguments = ["--override-keys", override_keys] if override_keys != None else []
    command = cmd_args(
        [
            processor,
            "process",
            "--input",
            input,
            "--output",
            output,
        ]
        + override_input_arguments
        + additional_keys_arguments
        + override_keys_arguments
    )
    ctx.actions.run(command, category = "apple_process_info_plist", identifier = action_id or input.basename, **_get_plist_run_options(ctx))

def _additional_keys_as_json_file(ctx: AnalysisContext) -> Artifact:
    additional_keys = _info_plist_additional_keys(ctx)
    return ctx.actions.write_json("plist_additional.json", additional_keys, has_content_based_path = False, exec_group = get_resources_execution_group(ctx))

def _info_plist_additional_keys(ctx: AnalysisContext) -> dict[str, typing.Any]:
    toolchain = get_resources_toolchain_info(ctx)
    sdk_name = toolchain.sdk_name
    platform_name = get_platform_name_for_sdk(sdk_name)
    sdk_metadata = get_apple_sdk_metadata_for_sdk_name(sdk_name)
    result = _extra_mac_info_plist_keys(sdk_metadata, ctx.attrs.extension)
    result["CFBundleSupportedPlatforms"] = sdk_metadata.info_plist_supported_platforms_values
    result["DTPlatformName"] = platform_name
    sdk_version = toolchain.sdk_version
    if sdk_version:
        platform_version = get_platform_version_for_sdk_version(
            sdk_name = sdk_name,
            sdk_version = sdk_version,
        )
        result["DTPlatformVersion"] = platform_version
        sdk_canonical_name = toolchain.sdk_canonical_name
        # TODO(T157757124): Remove conditional when selected Xcode provides this value
        if sdk_canonical_name:
            result["DTSDKName"] = sdk_canonical_name
    sdk_build_version = toolchain.sdk_build_version
    if sdk_build_version:
        result["DTPlatformBuild"] = sdk_build_version
        result["DTSDKBuild"] = sdk_build_version
    xcode_build_version = toolchain.xcode_build_version
    if xcode_build_version:
        result["DTXcodeBuild"] = xcode_build_version
    xcode_version = toolchain.xcode_version
    if xcode_version:
        result["DTXcode"] = xcode_version
    result[sdk_metadata.min_version_plist_info_key] = get_platform_version_for_sdk_version(
        sdk_name = sdk_name,
        sdk_version = get_bundle_min_target_version(ctx, get_default_binary_dep(ctx.attrs.binary)),
    )

    identify_build_system = ctx.attrs._info_plist_identify_build_system_default
    if ctx.attrs.info_plist_identify_build_system != None:
        identify_build_system = ctx.attrs.info_plist_identify_build_system
    if identify_build_system and ctx.attrs.extension == "app":
        # Only top-level .app bundle will contain special key.
        result["FBBuck2"] = True

    return result

def _extra_mac_info_plist_keys(sdk_metadata: AppleSdkMetadata, extension: str) -> dict[str, typing.Any]:
    if sdk_metadata.name == MacOSXSdkMetadata.name and extension != "xpc":
        return {
            "NSHighResolutionCapable": True,
            "NSSupportsAutomaticGraphicsSwitching": True,
        }
    else:
        return {}

def _override_keys_as_json_file(ctx: AnalysisContext) -> Artifact:
    override_keys = _info_plist_override_keys(ctx)
    return ctx.actions.write_json("plist_override.json", override_keys, has_content_based_path = False, exec_group = get_resources_execution_group(ctx))

def _info_plist_override_keys(ctx: AnalysisContext) -> dict[str, typing.Any]:
    sdk_name = get_resources_sdk_name(ctx)
    result = {}
    if sdk_name == MacOSXSdkMetadata.name:
        if ctx.attrs.extension != "xpc":
            result["LSRequiresIPhoneOS"] = False
    elif sdk_name in [WatchOSSdkMetadata.name, WatchSimulatorSdkMetadata.name]:
        result["UIDeviceFamily"] = [4]
        result["WKApplication"] = True
    elif sdk_name not in [MacOSXCatalystSdkMetadata.name]:
        result["LSRequiresIPhoneOS"] = True
    return result

def apple_info_plist_impl(ctx: AnalysisContext) -> list[Provider]:
    """
    Implementation for the apple_info_plist rule.

    This rule takes a source plist file and processes it to create an output plist.
    """
    apple_tools = ctx.attrs._apple_tools[AppleToolsInfo]
    processor = apple_tools.info_plist_processor

    input_plist = ctx.attrs.src
    output_plist = ctx.actions.declare_output("Info.plist", has_content_based_path = False)

    # Basic plist processing command
    command = cmd_args([
        processor,
        "process",
        "--input",
        input_plist,
        "--output",
        output_plist.as_output(),
    ])

    if ctx.attrs.xml:
        command = cmd_args(command, ["--output-xml"])

    # Add mutations if provided
    if ctx.attrs.mutations:
        mutations_file = ctx.actions.write_json(
            "mutations.json", ctx.attrs.mutations, has_content_based_path = False, exec_group = get_resources_execution_group(ctx)
        )
        command.add("--mutations")
        command.add(mutations_file)
        for mutation in ctx.attrs.mutations:
            operation = mutation[0]
            if operation in MergeOperations.values() or operation in RestrictedMergeOperations.values():
                command.add(cmd_args(hidden = mutation[1]))

    ctx.actions.run(command, category = "apple_info_plist", identifier = input_plist.basename, **_get_plist_run_options(ctx))

    return [
        DefaultInfo(default_output = output_plist),
    ]
