# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def strip_package_root(path: str, package_root: str) -> str:
    """Return `path` relative to `package_root`, the way `go list` prints it.

    Only a whole path component counts as the root: `server_testdata/x` is not
    below the root `server`, and is returned unchanged.
    """
    root = package_root.rstrip("/")
    if path == root:
        return ""
    if root != "" and path.startswith(root + "/"):
        return path[len(root) + 1 :].lstrip("/")
    return path.lstrip("/")
