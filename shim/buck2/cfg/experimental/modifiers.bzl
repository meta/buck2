# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# `PACKAGE` files load buck2's `cfg/experimental` as
# `@fbcode//buck2/cfg/experimental`, which is buck2's own `cfg/experimental`
# directory in open source.

load("@gh_facebook_buck2//cfg/experimental:modifiers.bzl", _modifiers = "modifiers")

modifiers = _modifiers
