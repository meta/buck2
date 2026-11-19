# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _toolchain(
    # starlark-lint-disable unused-argument
    ctx,
):  # @unused
    fail("we do not run analysis in query tests")

toolchain = rule(
    impl = _toolchain,
    attrs = {"dep": attrs.exec_dep()},
    is_toolchain_rule = True,
)
