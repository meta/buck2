# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:artifacts.bzl", "ArtifactExt")

ThirdPartyBuild = record(
    # A logical project name for the project, currently used for logging.
    project = field(str | None, None),
    # The directory containing the build output.
    root = field(ArtifactExt),
    # The prefix to install the build output.
    prefix = field(str, "/"),
    # A manifest of build env settings to use to build against this build.
    manifest = field(Artifact | None, None),
    # Environment variables to set to build against this project.
    # TODO(agallagher): Can this move into the manifest?
    exported_env = field(dict[str, str], {}),
)

# Work-around for buck2 bug causing "transitive values must be of the same
# transitive set type" errors:
# https://fb.prod.workplace.com/groups/buck2users/posts/3637287806527574/
ThirdPartyBuildTSet = transitive_set()
ThirdPartyBuildInfo = provider(
    fields = {
        "build": provider_field(ThirdPartyBuild | None),
        "_tset": provider_field(ThirdPartyBuildTSet),
    }
)

def third_party_build_info(
    actions, build: [ThirdPartyBuild, None] = None, children: list[ThirdPartyBuildInfo] = [], deps: list[Dependency] = []
) -> ThirdPartyBuildInfo:
    kwargs = {}
    if build != None:
        kwargs["value"] = build
    if deps or children:
        kwargs["children"] = [child._tset for child in children] + [dep[ThirdPartyBuildInfo]._tset for dep in deps if ThirdPartyBuildInfo in dep]
    return ThirdPartyBuildInfo(
        build = build,
        _tset = actions.tset(ThirdPartyBuildTSet, **kwargs),
    )
