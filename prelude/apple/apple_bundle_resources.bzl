# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:artifacts.bzl", "single_artifact")
load("@prelude//:paths.bzl", "paths")
load("@prelude//apple/swift:swift_toolchain_types.bzl", "SwiftToolchainInfo")
load("@prelude//cxx:headers.bzl", "CHeader")
load(
    "@prelude//linking:link_info.bzl",
    "CxxSanitizerRuntimeInfo",
)
load("@prelude//utils:utils.bzl", "flatten_dict")
load(
    ":apple_asset_catalog.bzl",
    "compile_apple_asset_catalog",
)
load(":apple_bundle_destination.bzl", "AppleBundleDestination")
load(":apple_bundle_part.bzl", "AppleBundleCodesignManifestTreePart", "AppleBundlePart", "AppleBundleSigningContextTreePart")
load(
    ":apple_bundle_types.bzl",
    "AppleBundleInfo",
    "AppleBundleTypeAppClip",
    "AppleBundleTypeDefault",
    "AppleBundleTypeExtensionKitExtension",
    "AppleBundleTypeWatchApp",
)
load(
    ":apple_bundle_utility.bzl",
    "get_bundle_resource_processing_options",
    "get_default_binary_dep",
    "get_extension_attr",
    "get_flattened_binary_deps",
    "get_is_watch_bundle",
    "get_product_name",
    "get_resources_execution_group",
    "get_resources_toolchain",
    "get_resources_toolchain_info",
)
load(":apple_core_data.bzl", "compile_apple_core_data")
load(":apple_info_plist.bzl", "process_info_plist", "process_plist")
load(":apple_library.bzl", "AppleLibraryForDistributionInfo")
load(":apple_library_types.bzl", "AppleLibraryInfo")
load(
    ":apple_resource_types.bzl",
    "AppleResourceDestination",
    "AppleResourceSelectionOutput",
    "AppleResourceSpec",  # @unused Used as a type
)
load(":apple_resource_utility.bzl", "apple_bundle_destination_from_resource_destination")
load(":modulemap.bzl", "create_modulemap")
load(
    ":resource_groups.bzl",
    "create_resource_graph",
    "get_filtered_resources",
    "get_resource_graph_node_map_func",
    "get_resource_group_info",
)
load(":scene_kit_assets.bzl", "compile_scene_kit_assets")
load(":xcassets_bundle_catalogs.bzl", "XcassetsBundleResourceCatalogs", "create_xcassets_bundle_resource_catalogs")
load(":xcassets_bundle_trim.bzl", "xcassets_trim_asset_catalog_specs")

AppleBundleResourcePartListOutput = record(
    # Resource parts to be copied into an Apple bundle, *excluding* binaries
    resource_parts = field(list[AppleBundlePart]),
    # Part that holds the info.plist
    info_plist_part = field(AppleBundlePart),
    # Parts which have an associated codesign manifest, it would contain
    # a _subset_ of the `resource_parts` parts.
    codesign_manifest_parts = field(list[AppleBundleCodesignManifestTreePart]),
    # Parts which have an associated signing context, it would contain
    # a _subset_ of the `resource_parts` parts.
    signing_context_parts = field(list[AppleBundleSigningContextTreePart]),
    xcassets_catalogs = field([XcassetsBundleResourceCatalogs, None], None),
    xcassets_sub_targets = field(dict[str, list[Provider]], {}),
    xcassets_trim_providers = field(list[Provider], []),
)

def get_apple_bundle_resource_part_list(ctx: AnalysisContext) -> AppleBundleResourcePartListOutput:
    parts = []

    parts.extend(_create_pkg_info_if_needed(ctx))
    parts.extend(_copy_privacy_manifest_if_needed(ctx))

    selection = _select_resources(ctx)

    # If we've pulled in native/C++ resources from deps, inline them into the
    # bundle under the `CxxResources` namespace.
    cxx_resources = flatten_dict([s.resources for s in selection.cxx_resource_specs])
    if cxx_resources:
        cxx_res_dir = ctx.actions.copied_dir(
            "CxxResources",
            {name: resource.default_output for name, resource in cxx_resources.items()},
            has_content_based_path = False,
        )
        selection.resource_specs.append(
            AppleResourceSpec(
                dirs = [cxx_res_dir],
                destination = AppleResourceDestination("resources"),
            ),
        )

    cxx_sanitizer_runtime_info = get_default_binary_dep(ctx.attrs.binary).get(CxxSanitizerRuntimeInfo) if ctx.attrs.binary else None
    if cxx_sanitizer_runtime_info and get_extension_attr(ctx) != "appex":
        runtime_resource_spec = AppleResourceSpec(
            files = cxx_sanitizer_runtime_info.runtime_files,
            destination = AppleResourceDestination("frameworks"),
            # Sanitizer dylibs require signing, for hardened runtime on macOS and iOS device builds
            codesign_files_on_copy = True,
        )
        selection.resource_specs.append(runtime_resource_spec)

    asset_catalog_specs, xcassets_sub_targets, xcassets_trim_providers = xcassets_trim_asset_catalog_specs(ctx, selection)
    asset_catalog_result = compile_apple_asset_catalog(ctx, asset_catalog_specs)
    if asset_catalog_result != None:
        asset_catalog_part = AppleBundlePart(
            source = asset_catalog_result.compiled_catalog,
            destination = AppleBundleDestination("resources"),
            # We only interested in directory contents
            new_name = "",
        )
        parts.append(asset_catalog_part)

    xcassets_catalogs = create_xcassets_bundle_resource_catalogs(
        ctx,
        selection,
        asset_catalog_result,
        lambda bundle_info: _copied_bundle_spec(bundle_info).destination,
    )

    extra_plist = asset_catalog_result.catalog_plist if asset_catalog_result != None else None
    info_plist_part = process_info_plist(ctx = ctx, override_input = extra_plist)

    core_data_result = compile_apple_core_data(ctx, selection.core_data_specs, get_product_name(ctx))
    if core_data_result != None:
        core_data_part = AppleBundlePart(
            source = core_data_result,
            destination = AppleBundleDestination("resources"),
            # We only interested in directory contents
            new_name = "",
        )
        parts.append(core_data_part)

    scene_kit_assets_result = compile_scene_kit_assets(ctx, selection.scene_kit_assets_spec)
    if scene_kit_assets_result != None:
        scene_kit_assets_part = AppleBundlePart(
            source = scene_kit_assets_result,
            destination = AppleBundleDestination("resources"),
            # We only interested in directory contents
            new_name = "",
        )
        parts.append(scene_kit_assets_part)

    parts.extend(_copy_resources(ctx, selection.resource_specs))
    first_level_parts, first_level_codesign_manifest_parts, first_level_signing_context_parts = _copy_first_level_bundles(ctx)
    parts.extend(first_level_parts)
    parts.extend(_copy_public_headers(ctx))
    parts.extend(_copy_module_map(ctx))
    parts.extend(_copy_swift_library_evolution_support(ctx))

    return AppleBundleResourcePartListOutput(
        resource_parts = parts,
        info_plist_part = info_plist_part,
        codesign_manifest_parts = first_level_codesign_manifest_parts,
        signing_context_parts = first_level_signing_context_parts,
        xcassets_catalogs = xcassets_catalogs,
        xcassets_sub_targets = xcassets_sub_targets,
        xcassets_trim_providers = xcassets_trim_providers,
    )

# Same logic as in v1, see `buck_client/src/com/facebook/buck/apple/ApplePkgInfo.java`
def _create_pkg_info_if_needed(ctx: AnalysisContext) -> list[AppleBundlePart]:
    extension = get_extension_attr(ctx)
    if extension == "xpc" or extension == "qlgenerator":
        return []
    artifact = ctx.actions.write("PkgInfo", "APPLWRUN\n", has_content_based_path = False, exec_group = get_resources_execution_group(ctx))
    return [AppleBundlePart(source = artifact, destination = AppleBundleDestination("metadata"))]

def _copy_privacy_manifest_if_needed(ctx: AnalysisContext) -> list[AppleBundlePart]:
    # `apple_test()` has no `privacy_manifest`, `copy_public_framework_headers` or `module_map`.
    privacy_manifest = getattr(ctx.attrs, "privacy_manifest", None)
    if privacy_manifest == None:
        return []

    # According to apple docs, privacy manifest has to be named as `PrivacyInfo.xcprivacy`
    if privacy_manifest.short_path.split("/", 1)[-1] == "PrivacyInfo.xcprivacy":
        artifact = privacy_manifest
    else:
        output = ctx.actions.declare_output("PrivacyInfo.xcprivacy", has_content_based_path = False)
        artifact = ctx.actions.copy_file(output.as_output(), privacy_manifest)
    return [AppleBundlePart(source = artifact, destination = AppleBundleDestination("resources"))]

def _select_resources(ctx: AnalysisContext) -> AppleResourceSelectionOutput:
    resource_group_info = get_resource_group_info(ctx)
    if resource_group_info:
        resource_groups_deps = resource_group_info.resource_group_to_implicit_deps_mapping.get(ctx.attrs.resource_group, []) if ctx.attrs.resource_group else []
        resource_group_mappings = resource_group_info.mappings
    else:
        resource_groups_deps = []
        resource_group_mappings = {}

    resource_graph = create_resource_graph(
        ctx = ctx,
        labels = [],
        bundle_binary = get_default_binary_dep(ctx.attrs.binary),
        deps = ctx.attrs.deps + resource_groups_deps,
        exported_deps = [],
    )
    resource_graph_node_map_func = get_resource_graph_node_map_func(resource_graph)
    return get_filtered_resources(ctx.label, resource_graph_node_map_func, ctx.attrs.resource_group, resource_group_mappings)

def _copy_swift_library_evolution_support(ctx: AnalysisContext) -> list[AppleBundlePart]:
    extension = get_extension_attr(ctx)
    if not extension == "framework":
        return []

    binary_deps = getattr(ctx.attrs, "binary")
    if binary_deps == None:
        return []

    swiftmodule_files = {}

    module_name = None
    for binary in get_flattened_binary_deps(binary_deps):
        apple_library_for_distribution_info = binary.get(AppleLibraryForDistributionInfo)
        if apple_library_for_distribution_info == None:
            continue
        module_name = apple_library_for_distribution_info.module_name
        if apple_library_for_distribution_info.swiftinterface != None:
            swiftmodule_files.update({
                apple_library_for_distribution_info.target_triple + ".swiftinterface": apple_library_for_distribution_info.swiftinterface,
            })
            if not getattr(ctx.attrs, "skip_private_swiftinterface", False):
                swiftmodule_files.update({
                    apple_library_for_distribution_info.target_triple
                    + ".private.swiftinterface": apple_library_for_distribution_info.private_swiftinterface,
                })
        if apple_library_for_distribution_info.swiftdoc != None:
            swiftmodule_files.update({
                apple_library_for_distribution_info.target_triple + ".swiftdoc": apple_library_for_distribution_info.swiftdoc,
            })

    if len(swiftmodule_files) == 0 or module_name == None:
        return []

    framework_module_dir = ctx.actions.declare_output(module_name + "framework.swiftmodule", dir = True, has_content_based_path = False)
    ctx.actions.copied_dir(framework_module_dir.as_output(), swiftmodule_files)
    return [AppleBundlePart(source = framework_module_dir, destination = AppleBundleDestination("modules"), new_name = module_name + ".swiftmodule")]

def _public_headers(ctx: AnalysisContext) -> list[Artifact]:
    if not getattr(ctx.attrs, "copy_public_framework_headers", None):
        return []
    binary_deps = getattr(ctx.attrs, "binary")
    if binary_deps == None:
        return []

    binary = get_default_binary_dep(binary_deps)
    apple_library_info = binary.get(AppleLibraryInfo)
    if apple_library_info == None:
        return []
    tset = apple_library_info.public_framework_headers

    headers = []
    if tset._tset:
        for public_framework_headers in tset._tset.traverse():
            for public_framework_header in public_framework_headers:
                for artifact in public_framework_header.artifacts:
                    headers.append(artifact)

    return headers

def _copy_public_headers(ctx: AnalysisContext) -> list[AppleBundlePart]:
    if not getattr(ctx.attrs, "copy_public_framework_headers", None):
        return []
    binary_deps = getattr(ctx.attrs, "binary")
    if binary_deps == None:
        return []

    binary = get_default_binary_dep(binary_deps)
    apple_library_info = binary.get(AppleLibraryInfo)
    if apple_library_info == None:
        return []
    tset = apple_library_info.public_framework_headers

    bundle_parts = []
    if tset._tset:
        for public_framework_headers in tset._tset.traverse():
            for public_framework_header in public_framework_headers:
                for artifact in public_framework_header.artifacts:
                    bundle_parts.append(AppleBundlePart(source = artifact, destination = AppleBundleDestination("headers")))

    if apple_library_info.swift_header:
        bundle_parts.append(AppleBundlePart(source = apple_library_info.swift_header, destination = AppleBundleDestination("headers")))

    return bundle_parts

def _create_framework_module_map(ctx: AnalysisContext) -> Artifact:
    binary = get_default_binary_dep(ctx.attrs.binary)
    apple_library_for_distribution_info = binary.get(AppleLibraryForDistributionInfo)
    if apple_library_for_distribution_info == None:
        fail("Tried to generate an automatic modulemap for an unsupported binary dep. Please make sure it is a modular apple_library")
    headers = _public_headers(ctx)
    cheaders = []
    for header in headers:
        cheaders.append(
            CHeader(
                artifact = header,
                name = header.basename,
                namespace = "",
                named = False,
            )
        )

    module_name = apple_library_for_distribution_info.module_name
    preprocessor_info = create_modulemap(
        ctx,
        name = "module",
        module_name = module_name,
        headers = cheaders,
        is_framework = True,
        swift_toolchain = get_resources_toolchain(ctx)[SwiftToolchainInfo],
        exec_group = get_resources_execution_group(ctx),
    )
    return preprocessor_info.modulemap_artifact

def _copy_module_map(ctx: AnalysisContext) -> list[AppleBundlePart]:
    extension = get_extension_attr(ctx)
    if not extension == "framework":
        return []
    module_map = getattr(ctx.attrs, "module_map", None)
    if module_map == None:
        return []
    if module_map == "auto":
        module_map = _create_framework_module_map(ctx)

    return [AppleBundlePart(source = module_map, destination = AppleBundleDestination("modules"))]

def _copy_resources(ctx: AnalysisContext, specs: list[AppleResourceSpec]) -> list[AppleBundlePart]:
    result = []

    for spec in specs:
        bundle_destination = apple_bundle_destination_from_resource_destination(spec.destination)
        result += [
            _process_apple_resource_file_if_needed(
                ctx = ctx,
                file = single_artifact(x).default_output,
                destination = bundle_destination,
                destination_relative_path = None,
                codesign_on_copy = spec.codesign_files_on_copy,
                codesign_entitlements = spec.codesign_entitlements,
                codesign_flags_override = spec.codesign_flags_override,
            )
            for x in spec.files
        ]
        result += _bundle_parts_for_dirs(spec.dirs, bundle_destination, False)
        result += _bundle_parts_for_dirs(spec.content_dirs, bundle_destination, True)
        result += _bundle_parts_for_variant_files(ctx, spec)

    return result

def _copy_first_level_bundles(
    ctx: AnalysisContext,
) -> (list[AppleBundlePart], list[AppleBundleCodesignManifestTreePart], list[AppleBundleSigningContextTreePart]):
    first_level_bundle_infos = filter(None, [dep.get(AppleBundleInfo) for dep in ctx.attrs.deps])

    bundle_parts = []
    codesign_manifest_parts = []
    signing_context_parts = []
    for bundle_info in first_level_bundle_infos:
        first_level_part = _copied_bundle_spec(bundle_info)
        if first_level_part != None:
            bundle_parts.append(first_level_part)
            if bundle_info.codesign_manifest_tree:
                codesign_manifest_parts.append(
                    AppleBundleCodesignManifestTreePart(
                        bundle_part = first_level_part,
                        codesign_manifest_tree = bundle_info.codesign_manifest_tree,
                    )
                )
            if bundle_info.signing_context_tree:
                signing_context_parts.append(
                    AppleBundleSigningContextTreePart(
                        bundle_part = first_level_part,
                        signing_context_tree = bundle_info.signing_context_tree,
                    )
                )

    return bundle_parts, codesign_manifest_parts, signing_context_parts

def _copied_bundle_spec(bundle_info: AppleBundleInfo) -> [None, AppleBundlePart]:
    bundle = bundle_info.bundle
    bundle_extension = paths.split_extension(bundle.short_path)[1]
    if bundle_extension == ".framework":
        destination = AppleBundleDestination("frameworks")
        codesign_on_copy = True
    elif bundle_extension == ".app":
        app_destination_type = "plugins"
        if bundle_info.bundle_type == AppleBundleTypeWatchApp:
            app_destination_type = "watchapp"
        elif bundle_info.bundle_type == AppleBundleTypeAppClip:
            app_destination_type = "appclips"
        elif bundle_info.bundle_type != AppleBundleTypeDefault:
            fail("Unhandled bundle type `{}`".format(bundle_info.bundle_type))
        destination = AppleBundleDestination(app_destination_type)
        codesign_on_copy = False
    elif bundle_extension == ".appex":
        # We have two types of extensions: App Extensions and ExtensionKit Extensions
        #
        # +----------------------+-------------------------------+-------------------------------+
        # |                      |        App Extension          |   ExtensionKit Extension      |
        # +----------------------+-------------------------------+-------------------------------+
        # | xcode project type   | com.apple.product-type.app-   | com.apple.product-type.       |
        # |                      | extension                     | extensionkit-extension        |
        # +----------------------+-------------------------------+-------------------------------+
        # | Info.plist           | NSExtensions                  | EXAppExtensionAttributes      |
        # +----------------------+-------------------------------+-------------------------------+
        # | bundle folder        | *.app/PlugIns                 | *.app/Extensions              |
        # +----------------------+-------------------------------+-------------------------------+
        #
        if bundle_info.bundle_type == AppleBundleTypeExtensionKitExtension:
            destination = AppleBundleDestination("extensionkit_extensions")
        else:
            destination = AppleBundleDestination("plugins")
        codesign_on_copy = False
    elif bundle_extension == ".qlgenerator":
        destination = AppleBundleDestination("quicklook")
        codesign_on_copy = True
    elif bundle_extension == ".xpc":
        destination = AppleBundleDestination("xpcservices")
        codesign_on_copy = True
    elif bundle_extension == ".bundle":
        destination = AppleBundleDestination("plugins")
        codesign_on_copy = True
    else:
        fail("Extension `{}` is not yet supported.".format(bundle_extension))
    return AppleBundlePart(
        source = bundle,
        destination = destination,
        codesign_on_copy = codesign_on_copy,
        extra_codesign_paths = bundle_info.extra_codesign_paths,
    )

def _bundle_parts_for_dirs(generated_dirs: list[Artifact], destination: AppleBundleDestination, copy_contents_only: bool) -> list[AppleBundlePart]:
    return [
        AppleBundlePart(
            source = generated_dir,
            destination = destination,
            new_name = "" if copy_contents_only else None,
        )
        for generated_dir in generated_dirs
    ]

def _bundle_parts_for_variant_files(ctx: AnalysisContext, spec: AppleResourceSpec) -> list[AppleBundlePart]:
    result = []

    # By definition, all variant files go into the resources destination
    bundle_destination = AppleBundleDestination("resources")
    for variant_file in spec.variant_files:
        variant_dest_subpath = _get_dest_subpath_for_variant_file(variant_file)
        bundle_part = _process_apple_resource_file_if_needed(
            ctx = ctx,
            file = variant_file,
            destination = bundle_destination,
            destination_relative_path = variant_dest_subpath,
        )
        result.append(bundle_part)

    for locale, variant_files in spec.named_variant_files.items():
        if not locale.endswith(".lproj"):
            fail("Keys for named variant files have to end with '.lproj' suffix, got {}".format(locale))
        result += [
            _process_apple_resource_file_if_needed(
                ctx = ctx,
                file = variant_file,
                destination = bundle_destination,
                destination_relative_path = paths.join(locale, variant_file.basename),
            )
            for variant_file in variant_files
        ]

    return result

def _run_ibtool(
    ctx: AnalysisContext,
    raw_file: Artifact,
    output: OutputArtifact,
    action_flags: list[str],
    target_device: [None, str],
    command_for_identifier: str,
    output_is_dir: bool,
) -> None:
    # TODO(T110378103): detect and add minimum deployment target automatically
    # TODO(T110378113): add support for ibtool modules (turned on by `ibtool_module_flag` field of `apple_bundle` rule)

    # Equivalent of `AppleProcessResources::BASE_IBTOOL_FLAGS` from v1
    base_flags = ["--output-format", "human-readable-text", "--notices", "--warnings", "--errors"]
    ibtool = get_resources_toolchain_info(ctx).ibtool
    ibtool_flags = getattr(ctx.attrs, "ibtool_flags", None) or []
    ibtool_command = [ibtool] + base_flags + ibtool_flags
    if target_device != None:
        ibtool_command.extend(["--target-device", target_device])
    ibtool_command.extend(action_flags)
    if output_is_dir:
        ibtool_command.append('"$TMPDIR"')
        copy_back = cmd_args(output, format = 'mkdir -p {} && cp -R "$TMPDIR"/. {}')
    else:
        output_for_tool_invocation = '"$TMPDIR"/"' + output.basename + '"'
        ibtool_command.append(output_for_tool_invocation)
        copy_back = cmd_args(output, format = "cp " + output_for_tool_invocation + " {}")
    ibtool_command.append(cmd_args(raw_file, format = '"$EXEC_ROOT"/{}'))

    # Sandboxing and fs isolation on RE machines results in Xcode tools failing
    # when those are working in freshly created directories in buck-out.
    # See https://fb.workplace.com/groups/1042353022615812/permalink/1872164996301273/
    # As a workaround create a directory in tmp, use it for Xcode tools, then
    # copy the result to buck-out.
    script_lines = [
        cmd_args("set -euo pipefail"),
        cmd_args('EXEC_ROOT="$PWD"'),
        cmd_args('TMPDIR="$(mktemp -d)"'),
        cmd_args(ibtool_command, delimiter = " "),
        copy_back,
    ]

    wrapper_script, _ = ctx.actions.write(
        "ibtool_wrapper_" + _wrapper_suffix(command_for_identifier, raw_file) + ".sh",
        script_lines,
        allow_args = True,
        has_content_based_path = False,
        exec_group = get_resources_execution_group(ctx),
    )
    command = cmd_args(["/bin/sh", wrapper_script], hidden = [ibtool_command, output])
    action_identifier = _ibtool_identifier(command_for_identifier, raw_file)
    processing_options = get_bundle_resource_processing_options(ctx)
    ctx.actions.run(
        command,
        prefer_local = processing_options.prefer_local,
        prefer_remote = processing_options.prefer_remote,
        allow_cache_upload = processing_options.allow_cache_upload,
        exec_group = get_resources_execution_group(ctx),
        category = "apple_ibtool",
        identifier = action_identifier,
    )

def _identifier_parts(raw_file: Artifact) -> list[str]:
    identifier_parts = []
    variant_dirname = _get_variant_dirname(raw_file)
    if variant_dirname:
        # variant_name is like "zh_TW.lproj", and we only want "zh_TW"
        variant_name = paths.replace_extension(variant_dirname, "")
        identifier_parts.append(variant_name)
    identifier_parts += [raw_file.basename]
    return identifier_parts

def _ibtool_identifier(action: str, raw_file: Artifact) -> str:
    "*.xib files can live in .lproj folders and have the same name, so we need to split the id"
    identifier_parts = _identifier_parts(raw_file)
    return "ibtool_" + action + " " + "/".join(identifier_parts)

def _wrapper_suffix(action: str, raw_file: Artifact) -> str:
    return "_".join([action] + _identifier_parts(raw_file))

def _compile_ui_resource(
    ctx: AnalysisContext, raw_file: Artifact, output: OutputArtifact, target_device: [None, str] = None, output_is_dir: bool = False
) -> None:
    _run_ibtool(
        ctx = ctx,
        raw_file = raw_file,
        output = output,
        action_flags = ["--compile"],
        target_device = target_device,
        command_for_identifier = "compile",
        output_is_dir = output_is_dir,
    )

def _link_ui_resource(ctx: AnalysisContext, raw_file: Artifact, output: OutputArtifact, target_device: [None, str] = None, output_is_dir: bool = False) -> None:
    _run_ibtool(
        ctx = ctx,
        raw_file = raw_file,
        output = output,
        action_flags = ["--link"],
        target_device = target_device,
        command_for_identifier = "link",
        output_is_dir = output_is_dir,
    )

def _process_apple_resource_file_if_needed(
    ctx: AnalysisContext,
    file: Artifact,
    destination: AppleBundleDestination,
    destination_relative_path: [str, None],
    codesign_on_copy: bool = False,
    codesign_entitlements: Artifact | None = None,
    codesign_flags_override: list[str] | None = None,
) -> AppleBundlePart:
    output_dir = "_ProcessedResources"
    basename = file.basename
    output_is_contents_dir = False
    if basename.endswith(".plist") or basename.endswith(".stringsdict"):
        processed = ctx.actions.declare_output(paths.join(output_dir, file.short_path), has_content_based_path = False)
        process_plist(
            ctx = ctx,
            input = file,
            output = processed.as_output(),
            action_id = destination_relative_path,
        )
    elif basename.endswith(".storyboard"):
        if destination_relative_path:
            destination_relative_path = paths.replace_extension(destination_relative_path, ".storyboardc")
        compiled = ctx.actions.declare_output(
            paths.join(output_dir, paths.replace_extension(file.short_path, ".storyboardc")), dir = True, has_content_based_path = False
        )
        if get_is_watch_bundle(ctx):
            output_is_contents_dir = True
            _compile_ui_resource(ctx = ctx, raw_file = file, output = compiled.as_output(), target_device = "watch", output_is_dir = True)
            processed = ctx.actions.declare_output(
                paths.join(output_dir, paths.replace_extension(file.short_path, "_linked_storyboard")), dir = True, has_content_based_path = False
            )
            _link_ui_resource(ctx = ctx, raw_file = compiled, output = processed.as_output(), target_device = "watch", output_is_dir = True)
        else:
            processed = compiled
            _compile_ui_resource(ctx, file, processed.as_output(), output_is_dir = True)
    elif basename.endswith(".xib"):
        if destination_relative_path:
            destination_relative_path = paths.replace_extension(destination_relative_path, ".nib")
        processed = ctx.actions.declare_output(paths.join(output_dir, paths.replace_extension(file.short_path, ".nib")), has_content_based_path = False)
        _compile_ui_resource(ctx, file, processed.as_output())
    else:
        processed = file

    # When name is empty string only content of the directory will be copied, as opposed to the directory itself.
    # When name is `None`, directory or file will be copied as it is, without renaming.
    new_name = destination_relative_path if destination_relative_path else ("" if output_is_contents_dir else None)
    return AppleBundlePart(
        source = processed,
        destination = destination,
        new_name = new_name,
        codesign_on_copy = codesign_on_copy,
        codesign_entitlements = codesign_entitlements,
        codesign_flags_override = codesign_flags_override,
    )

# Returns a path relative to the _parent_ of the lproj dir.
# For example, given a variant file with a short path of`XX/YY.lproj/ZZ`
# it would return `YY.lproj/ZZ`.
def _get_dest_subpath_for_variant_file(variant_file: Artifact) -> str:
    dir_name = _get_variant_dirname(variant_file)
    if not dir_name:
        fail("Variant files have to be in a directory with name ending in '.lproj' but `{}` was not.".format(variant_file.short_path))
    file_name = variant_file.basename
    return paths.join(dir_name, file_name)

def _get_variant_dirname(variant_file: Artifact) -> str | None:
    dir_name = paths.basename(paths.dirname(variant_file.short_path))
    return dir_name if dir_name.endswith("lproj") else None
