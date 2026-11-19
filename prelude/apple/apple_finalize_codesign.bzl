# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//xplugins:types.bzl", "XPluginsFunctionMappingManifestInfo")
load(
    ":apple_bundle_types.bzl",
    "AppleBundleInfo",
    "AppleBundleLinkerMapInfo",
    "AppleInfoPlistInfo",
)
load(
    ":debug.bzl",
    "AppleDebuggableInfo",
)

def _apple_finalize_bundle_impl(ctx):
    original_bundle = ctx.attrs.bundle
    original_bundle_info = original_bundle[AppleBundleInfo]
    bundle_artifact = original_bundle_info.bundle
    finalized_bundle = ctx.actions.declare_output(bundle_artifact.basename, has_content_based_path = False)

    cmd = cmd_args([
        ctx.attrs.finalizer[RunInfo],
        "--input-bundle-path",
        bundle_artifact,
        "--output-bundle-path",
        finalized_bundle.as_output(),
        "--sign-key",
        ctx.attrs.sign_key,
    ])
    ctx.actions.run(
        cmd,
        category = "apple_finalize_bundle",
        identifier = bundle_artifact.basename,
    )

    finalized_bundle_info = AppleBundleInfo(
        bundle = finalized_bundle,
        bundle_type = original_bundle_info.bundle_type,
        binary_name = original_bundle_info.binary_name,
        contains_watchapp = original_bundle_info.contains_watchapp,
        skip_copying_swift_stdlib = original_bundle_info.skip_copying_swift_stdlib,
        signing_info = original_bundle_info.signing_info,
    )

    forwarded_providers = [original_bundle[AppleDebuggableInfo], original_bundle[AppleInfoPlistInfo], original_bundle[AppleBundleLinkerMapInfo]]
    xplugins_function_mapping_manifest_info = original_bundle.get(XPluginsFunctionMappingManifestInfo)
    if xplugins_function_mapping_manifest_info:
        forwarded_providers.append(xplugins_function_mapping_manifest_info)

    return [
        DefaultInfo(default_output = finalized_bundle),
        finalized_bundle_info,
    ] + forwarded_providers

apple_finalize_bundle = rule(
    attrs = {
        "bundle": attrs.dep(providers = [AppleBundleInfo]),
        "finalizer": attrs.exec_dep(providers = [RunInfo]),
        "sign_key": attrs.string(default = "fbios-debug"),
    },
    impl = _apple_finalize_bundle_impl,
)
