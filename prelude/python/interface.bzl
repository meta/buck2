# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

PythonLibraryManifestsInterface = record(
    # Returns the source manifests for this Python library.
    # [_arglike] of source manifests
    src_manifests = field(typing.Callable),
    # Returns the files referenced by source manifests for this Python library.
    # [_arglike] of source artifacts
    src_artifacts = field(typing.Callable),
    # Returns the bytecode manifests for this Python library.
    # [_arglike] of bytecode manifests
    bytecode_manifests = field(typing.Callable),
    # Returns the files referenced by bytecode manifests for this Python library.
    # [_arglike] of bytecode artifacts
    bytecode_artifacts = field(typing.Callable),
    # Returns the resources manifests for this Python library.
    # [_arglike] of resource manifests
    resource_manifests = field(typing.Callable),
    # Returns the files referenced by resource manifests for this Python library.
    # [_arglike] of resource artifacts
    resource_artifacts = field(typing.Callable),
    hidden_resources = field(typing.Callable),
    has_hidden_resources = field(typing.Callable),
)

# Entry point for Python binaries. First component designates if the second
# component is to be interpreted as a module or a function name.
EntryPointKind = enum("module", "function")
EntryPoint = (EntryPointKind, str)
