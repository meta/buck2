# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def write_hmap(actions: AnalysisActions, output: Artifact, headers: dict[str, (Artifact, str)]) -> Artifact | None:
    """Write a Clang header map if this Buck binary supports the action.

    Each entry maps an include name to `(artifact, path)`, where `path` is
    relative to the artifact and empty for the artifact itself.
    Load this function instead of calling `ctx.actions._write_hmap` directly.
    The implementation may move back into the prelude in the future.
    Returns None on older Buck binaries so callers can use another action.
    """
    native_write_hmap = getattr(actions, "_write_hmap", None)
    if native_write_hmap == None:
        return None
    return native_write_hmap(output, headers)
