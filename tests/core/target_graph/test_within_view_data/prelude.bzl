# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

test_within_view_rule = rule(
    impl = lambda ctx: fail("we don't run analysis in this test, {}".format(ctx)),
    attrs = {
        "deps": attrs.list(attrs.dep()),
    },
)

test_within_view_defaults_rule = rule(
    impl = lambda ctx: fail("we don't run analysis in this test, {}".format(ctx)),
    attrs = {
        "defaulted": attrs.dep(default = "//a:a"),
    },
)
