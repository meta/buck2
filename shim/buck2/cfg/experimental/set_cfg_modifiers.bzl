# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# `PACKAGE` files load buck2's `cfg/experimental` as
# `@fbcode//buck2/cfg/experimental`, which is this directory in open source.
# The prelude's `set_cfg_modifiers` must be called from the `PACKAGE` file
# itself (it checks the call stack), so this re-exports it rather than
# wrapping it.

load("@prelude//cfg/modifier:set_cfg_modifiers.bzl", _set_cfg_modifiers = "set_cfg_modifiers")

set_cfg_modifiers = _set_cfg_modifiers
