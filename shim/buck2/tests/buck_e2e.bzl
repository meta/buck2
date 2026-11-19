# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# The e2e tests' `BUCK` files load their macros from `@fbcode//buck2/tests`,
# which is buck2's own `tests` directory in open source.
load(
    "@gh_facebook_buck2//tests:buck_e2e.bzl",
    _buck2_core_tests = "buck2_core_tests",
    _buck2_e2e_test = "buck2_e2e_test",
)

buck2_core_tests = _buck2_core_tests
buck2_e2e_test = _buck2_e2e_test
