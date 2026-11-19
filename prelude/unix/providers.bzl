# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:artifacts.bzl", "ArtifactExt")
load("@prelude//linking:shared_libraries.bzl", "SharedLibraries")
load("@prelude//python:manifest.bzl", "ManifestInfo")
load("@prelude//python:python.bzl", "PythonLibraryInfo")

# Provider representing components that can be added to a "unix" env (e.g.
# binaries in `bin/`, native libs in `lib/`, and Python modules under
# `lib/python*/site-packages`).
UnixEnv = record(
    label = field(Label),
    # Third-party builds to install into the env (non-transitive).
    third_party_builds = field(list[ArtifactExt], []),
    # Python libraries to install (non-transitive).
    python_libs = field(list[PythonLibraryInfo], []),
    # Native libs to install (non-transitive).
    native_libs = field(list[SharedLibraries], []),
    # Binaries to install.
    binaries = field(list[ManifestInfo], []),
    # Raw paths to install.
    paths = field(list[(str, ArtifactExt)], []),
    patterns = field(list[(str, ArtifactExt, str)], []),
)

UnixEnvTSet = transitive_set()

UnixEnvInfo = provider(
    fields = dict(
        _tset = provider_field(UnixEnvTSet),
    ),
)

def create_unix_env_info(actions: AnalysisActions, env: UnixEnv | None = None, children: list[UnixEnvInfo] = [], deps: list[Dependency] = []) -> UnixEnvInfo:
    all_children = []
    for child in children:
        all_children.append(child._tset)
    for dep in deps:
        child = dep.get(UnixEnvInfo)
        if child != None:
            all_children.append(child._tset)
    kwargs = {}
    if env != None:
        kwargs["value"] = env
    kwargs["children"] = all_children
    return UnixEnvInfo(
        _tset = actions.tset(UnixEnvTSet, **kwargs),
    )
