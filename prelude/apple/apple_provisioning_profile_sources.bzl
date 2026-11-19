# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:artifacts.bzl", "ArtifactGroupInfo")

AppleProvisioningProfileSourceInfo = record(
    # Individual profile files, populated when the source is a filegroup
    # that provides ArtifactGroupInfo. None means this source uses a
    # directory instead. An empty list means the source has no profiles.
    profiles = field([list[Artifact], None], None),
    # A directory containing profiles, populated when the source is a
    # genrule that only provides DefaultInfo (e.g. compressed profiles
    # decompressed at build time).
    directory = field([Artifact, None], None),
)

AppleProvisioningProfileSourcesInfo = provider(
    fields = {
        "sources": provider_field(list[AppleProvisioningProfileSourceInfo]),
    },
)

def apple_provisioning_profile_sources_impl(ctx: AnalysisContext) -> list[Provider]:
    sources = []
    for dep in ctx.attrs.sources:
        artifact_group_info = dep.get(ArtifactGroupInfo)
        if artifact_group_info:
            # This handles the case where a filegroup defines the set of profiles
            sources.append(
                AppleProvisioningProfileSourceInfo(
                    profiles = artifact_group_info.artifacts,
                )
            )
        else:
            # This handles the case where a genrule produces a directory of profiles
            default_info = dep[DefaultInfo]
            if default_info.default_outputs:
                sources.append(
                    AppleProvisioningProfileSourceInfo(
                        directory = default_info.default_outputs[0],
                    )
                )
    return [
        DefaultInfo(default_outputs = []),
        AppleProvisioningProfileSourcesInfo(
            sources = sources,
        ),
    ]
