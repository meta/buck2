# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    ":apple_asset_catalog_types.bzl",
    "AppleAssetCatalogResult",  # @unused Used as a type
)
load(
    ":apple_bundle_destination.bzl",
    "AppleBundleDestination",  # @unused Used as a type
)
load(
    ":apple_bundle_types.bzl",
    "AppleBundleInfo",  # @unused Used as a type
)
load(":apple_resource_types.bzl", "AppleResourceSelectionOutput")

XcassetsBundleResourceCatalogs = record()

def create_xcassets_bundle_resource_catalogs(
    _ctx: AnalysisContext,
    _selection: AppleResourceSelectionOutput,
    _asset_catalog_result: [AppleAssetCatalogResult, None],
    _child_bundle_destination: typing.Callable[[AppleBundleInfo], AppleBundleDestination],
) -> [XcassetsBundleResourceCatalogs, None]:
    return None

def xcassets_bundle_catalogs_providers(_ctx: AnalysisContext, _resource_catalogs: [XcassetsBundleResourceCatalogs, None]) -> list[Provider]:
    return []
