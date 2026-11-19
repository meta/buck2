# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:paths.bzl", "paths")
load("@prelude//dist:dist_info.bzl", "DistInfo")
load("@prelude//utils:arglike.bzl", "ArgLike")  # @unused Used as a type
load("@prelude//utils:expect.bzl", "expect")

# A group of artifacts.
ArtifactGroupInfo = provider(
    fields = {
        "artifacts": provider_field(typing.Any, default = None),  # ["artifact"]
    },
)

ArtifactOutputs = record(
    # Single output. This is the artifact whose path would go into the resources
    # JSON when this artifact is used as a resource.
    default_output = field(Artifact),
    # Other artifacts which need to be present in order to run the resource as
    # an executable. This includes shared library dependencies and resources.
    nondebug_runtime_files = field(list[ArgLike]),
    # Other outputs that would be materialized if this artifact is the output of
    # a build, or generally in any context where a user might run this artifact
    # in a debugger.
    #
    # This is a superset of nondebug_runtime_files and also includes external
    # debuginfo.
    other_outputs = field(list[ArgLike]),
)

# Wrapper to support wrapping `Artifact`s referencing paths behind external
# symlinks.
ArtifactExt = record(
    artifact = field(Artifact),
    # If the `artifact` above is a symlink referencing an external path, this
    # is an optional sub-path to append when accessing the path.
    sub_path = field(str | None, None),
    # Returns the resolved path as a `cmd_arg()`, with the optional sub-path
    # appended.
    as_arg = field(typing.Callable),
    join = field(typing.Callable),
)

# A Provider that mirrors `DefaultInfo` for `Artifact` outputs, but allows
# specifying an `ArtifactExt` as it's default output.
DefaultOutputExt = provider(
    fields = dict(
        default_output = provider_field(ArtifactExt),
    ),
)

def single_artifact(dep: Artifact | Dependency) -> ArtifactOutputs:
    if isinstance(dep, Artifact):
        return ArtifactOutputs(
            default_output = dep,
            nondebug_runtime_files = [],
            other_outputs = [],
        )

    if DefaultInfo in dep:
        info = dep[DefaultInfo]
        expect(
            len(info.default_outputs) == 1,
            "expected exactly one default output from {} ({})".format(dep, info.default_outputs),
        )
        default_output = info.default_outputs[0]
        other_outputs = info.other_outputs

        dist_info = dep.get(DistInfo)
        nondebug_runtime_files = dist_info.nondebug_runtime_files if dist_info else other_outputs

        return ArtifactOutputs(
            default_output = default_output,
            nondebug_runtime_files = nondebug_runtime_files,
            other_outputs = other_outputs,
        )

    fail("unexpected dependency type: {}".format(type(dep)))

def unpack_artifacts(artifacts: list[Artifact | Dependency]) -> list[ArtifactOutputs]:
    """
    Unpack a heterogeneous list of Artifact and ArtifactGroupInfo into a list
    representing their outputs.
    """

    out = []

    for artifact in artifacts:
        if isinstance(artifact, Artifact):
            out.append(
                ArtifactOutputs(
                    default_output = artifact,
                    nondebug_runtime_files = [],
                    other_outputs = [],
                )
            )
            continue

        if ArtifactGroupInfo in artifact:
            for artifact in artifact[ArtifactGroupInfo].artifacts:
                out.append(
                    ArtifactOutputs(
                        default_output = artifact,
                        nondebug_runtime_files = [],
                        other_outputs = [],
                    )
                )
            continue

        out.append(single_artifact(artifact))

    return out

def unpack_artifact_map(artifacts: dict[str, Artifact | Dependency]) -> dict[str, ArtifactOutputs]:
    """
    Unpack a heterogeneous dict of Artifact and ArtifactGroupInfo into a dict
    representing their outputs.
    """

    out = {}

    for name, artifact in artifacts.items():
        if isinstance(artifact, Artifact):
            out[name] = ArtifactOutputs(
                default_output = artifact,
                nondebug_runtime_files = [],
                other_outputs = [],
            )
            continue

        if ArtifactGroupInfo in artifact:
            for artifact in artifact[ArtifactGroupInfo].artifacts:
                out[paths.join(name, artifact.short_path)] = ArtifactOutputs(
                    default_output = artifact,
                    nondebug_runtime_files = [],
                    other_outputs = [],
                )
            continue

        out[name] = single_artifact(artifact)

    return out

def _as_arg(artifact: Artifact, sub_path: str | None) -> ArgLike:
    if sub_path == None:
        return artifact
    return cmd_args(artifact, format = "{{}}/{}".format(sub_path))

def artifact_ext(artifact: Artifact, sub_path: str | None = None) -> ArtifactExt:
    return ArtifactExt(
        artifact = artifact,
        sub_path = sub_path,
        as_arg = lambda: _as_arg(artifact, sub_path),
        join = lambda p: artifact_ext(
            artifact = artifact,
            sub_path = p if sub_path == None else paths.join(sub_path, p),
        ),
    )

def to_artifact_ext(src: Artifact | Dependency) -> ArtifactExt:
    if isinstance(src, Dependency):
        ext = src.get(DefaultOutputExt)
        if ext != None:
            return ext.default_output
        else:
            (src,) = src[DefaultInfo].default_outputs
    return artifact_ext(src)

def to_arglike(src: Artifact | Dependency) -> ArgLike:
    return to_artifact_ext(src).as_arg()
