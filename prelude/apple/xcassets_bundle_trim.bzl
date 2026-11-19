# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# @oss-disable[end= ]: load(
    # @oss-disable[end= ]: "@prelude//apple/meta_only:xcassets_bundle_trim.bzl",
    # @oss-disable[end= ]: _trim_asset_catalog_specs = "meta_xcassets_trim_asset_catalog_specs",
# @oss-disable[end= ]: )
load(
    ":apple_asset_catalog_types.bzl",
    "AppleAssetCatalogSpec",  # @unused Used as a type
)
load(":apple_resource_types.bzl", "AppleResourceSelectionOutput")

def xcassets_trim_asset_catalog_specs(
    ctx: AnalysisContext, selection: AppleResourceSelectionOutput
) -> (list[AppleAssetCatalogSpec], dict[str, list[Provider]], list[Provider]):
    """Returns the asset catalog specs to compile, sub-targets describing the change, and providers for the bundle."""
    # @oss-disable[end= ]: return _trim_asset_catalog_specs(ctx, selection)
    return selection.asset_catalog_specs, {}, [] # @oss-enable
