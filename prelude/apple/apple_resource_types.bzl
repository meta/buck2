# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:artifacts.bzl", "ArtifactOutputs")
load(":apple_asset_catalog_types.bzl", "AppleAssetCatalogSpec")
load(":apple_core_data_types.bzl", "AppleCoreDataSpec")
load(":scene_kit_assets_types.bzl", "SceneKitAssetsSpec")
load(":xcassets_asset_symbols.bzl", "MetaXcassetsAssetSymbolSpec")

# Represents the values for the `destination` field of `apple_resource`
#
# Not every destination is meaningful for every bundle kind: like `loginitems`, `launchagents`
# only has a path in macOS application bundles, and resolves to the bundle root elsewhere.
AppleResourceDestination = enum(
    "executables",
    "extensionkit_extensions",
    "frameworks",
    "launchagents",
    "launchdaemons",
    "loginitems",
    "plugins",
    "resources",
    "systemextensions",
    "xpcservices",
)

# Defines _where_ resources need to be placed in an `apple_bundle`
AppleResourceSpec = record(
    files = field(list[[Artifact, Dependency]], []),
    dirs = field(list[Artifact], []),
    content_dirs = field(list[Artifact], []),
    destination = AppleResourceDestination,
    variant_files = field(list[Artifact], []),
    # Map from locale to list of files for that locale, e.g.
    # `{ "ru.lproj" : ["Localizable.strings"] }`
    named_variant_files = field(dict[str, list[Artifact]], {}),
    codesign_files_on_copy = field(bool, False),
    codesign_entitlements = field(Artifact | None, None),
    codesign_flags_override = field(list[str] | None, None),
)

# Used when invoking `ibtool`, `actool`, `mapc` and `momc`
AppleResourceProcessingOptions = record(
    prefer_local = field(bool, False),
    prefer_remote = field(bool, False),
    allow_cache_upload = field(bool, False),
)

CxxResourceSpec = record(
    resources = field(dict[str, ArtifactOutputs], {}),
)

# A selected asset catalog with the resource-graph target that provided it.
SelectedAppleAssetCatalogSpec = record(
    asset_catalog_spec = field(AppleAssetCatalogSpec),
    xcassets_symbol_spec = field([MetaXcassetsAssetSymbolSpec, None], None),
    target = field(Label),
)

# An asset symbol spec on a node of the bundle's resource graph, and whether
# the bundle's resource group selects that node.
ReachableXcassetsSymbolSpec = record(
    spec = field(MetaXcassetsAssetSymbolSpec),
    selected = field(bool),
)

AppleResourceSelectionOutput = record(
    resource_specs = field(list[AppleResourceSpec]),
    asset_catalog_specs = field(list[AppleAssetCatalogSpec]),
    selected_asset_catalog_specs = field(list[SelectedAppleAssetCatalogSpec], []),
    reachable_xcassets_symbol_specs = field(list[ReachableXcassetsSymbolSpec], []),
    core_data_specs = field(list[AppleCoreDataSpec]),
    scene_kit_assets_spec = field(list[SceneKitAssetsSpec]),
    cxx_resource_specs = field(list[CxxResourceSpec]),
)
