# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//apple/user:apple_selective_debugging.bzl", "AppleSelectedDebugInfo")
load(":debug.bzl", "AppleDebuggableInfo")

AppleBundleType = enum(
    "default",
    # Bundle was built for watchOS Apple platform
    "watchapp",
    # Bundle represents an App Clip to be embedded
    "appclip",
    # Bundle represents an ExtensionKit extension to be embedded
    "extensionkit_extension",
)

ApplePackageExtension = enum(
    "ipa",
    "pkg",
    "dmg",
    "zip",
)

AppleBundleManifestLogFiles = record(
    command_file = field(Artifact),
    spec_file = field(Artifact),
    log_file = field([Artifact, None], None),
)

AppleBundleManifest = record(
    log_file_map = dict[Label, AppleBundleManifestLogFiles],
)

AppleBundleCodesignManifestTree = record(
    # Codesign manifest for the bundle itself
    codesign_manifest = field(Artifact),
    # Codesign manifest for inner bundles.
    # Maps from relative bundle destination -> AppleBundleCodesignManifestTree
    inner_codesign_manifests = field(dict[str, typing.Any]),
)

AppleBundleSigningContextTree = record(
    # Signing context for the bundle itself
    signing_context = field(Artifact),
    # Signing contexts for inner bundles.
    # Maps from relative bundle destination -> AppleBundleSigningContextTree
    inner_signing_contexts = field(dict[str, typing.Any]),
)

AppleBundleManifestInfo = provider(
    fields = {
        "manifest": provider_field(AppleBundleManifest),
    },
)

# Provider flagging that result of the rule contains Apple bundle.
# It might be copied into main bundle to appropriate place if rule
# with this provider is a dependency of `apple_bundle`.
AppleBundleInfo = provider(
    # @unsorted-dict-items
    fields = {
        # Result bundle
        "bundle": provider_field(Artifact),
        "bundle_type": provider_field(AppleBundleType),
        # The name of the executable within the bundle.
        "binary_name": provider_field([str, None], default = None),
        # If the bundle contains a Watch bundle, we have to update the packaging.
        # Similar to `is_watchos`, this might be omitted for certain types of bundles which don't depend on it.
        "contains_watchapp": provider_field([bool, None]),
        # By default, non-framework, non-appex binaries copy Swift libraries into the final
        # binary. This is the opt-out for that.
        "skip_copying_swift_stdlib": provider_field([bool, None]),
        # List of extra paths (relative to bundle root) to be codesigned.
        "extra_codesign_paths": provider_field([list[str], None], default = None),
        "codesign_manifest_tree": provider_field(AppleBundleCodesignManifestTree | None, default = None),
        "signing_context_tree": provider_field(AppleBundleSigningContextTree | None, default = None),
        "signing_info": provider_field(Artifact | None, default = None),
    },
)

# Provider which helps to propagate minimum deployment version up the target graph.
AppleMinDeploymentVersionInfo = provider(
    fields = {
        "version": provider_field([str, None]),
    }
)

AppleBundleResourceInfo = provider(
    fields = {
        "resource_output": provider_field(typing.Any, default = None),  # AppleBundleResourcePartListOutput
    }
)

AppleBundleLinkerMapInfo = provider(
    fields = {
        "linker_maps": provider_field(list[Artifact]),
    }
)

# Providers used to merge extra linker outputs as a top level output
# of an application bundle.
AppleBinaryExtraOutputsInfo = provider(
    fields = {
        "default_output": provider_field(Artifact),
        "extra_outputs": provider_field(dict[str, list[Artifact]]),
        "name": provider_field(str),
    }
)

AppleBundleExtraOutputsInfo = provider(
    fields = {
        "extra_outputs": provider_field(list[AppleBinaryExtraOutputsInfo]),
    }
)

AppleBundleBinaryOutput = record(
    binary = field(Artifact),
    # If `unstripped_binary` is `None`, then `binary` is unstripped.
    # Otherwise, `unstripped_binary` is distinct from `binary` and is unstripped.
    unstripped_binary = field([Artifact, None]),
    debuggable_info = field([AppleDebuggableInfo, None], None),
    # When set, the binary is not scrubbed and its dSYMs must be generated
    # with this `dsymutil --allow` file instead.
    dsym_allowlist = field([Artifact, None], None),
    # When set, the dSYMs of the binary must be generated from these artifacts
    # instead of `debuggable_info.debug_info_tset`.
    selected_debug_info = field([AppleSelectedDebugInfo, None], None),
)

AppleBundleTypeDefault = AppleBundleType("default")
AppleBundleTypeWatchApp = AppleBundleType("watchapp")
AppleBundleTypeAppClip = AppleBundleType("appclip")
AppleBundleTypeExtensionKitExtension = AppleBundleType("extensionkit_extension")

# Represents the user-visible type which is distinct from the internal one (`AppleBundleType`)
AppleBundleTypeAttributeType = enum(
    "appclip",
    "extensionkit_extension",
    "watchapp",
)

AppleInfoPlistInfo = provider(
    fields = {
        "info_plist": provider_field(Artifact),
    }
)
