# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# A no-op rather than the prelude's `with_rust_workspace`: the workspace
# targets a `PACKAGE` file names are fbsource labels, which would end up in the
# `_workspaces` of every Rust target below it.

def with_rust_workspace(targets):  # @unused
    pass
