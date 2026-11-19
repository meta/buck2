# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Rule with no attrs that may produce an output non-deterministically based on the path provided in buckconfig
def _non_det_build(ctx):
    return [DefaultInfo(default_output = ctx.actions.write("foo.txt", ctx.attrs.buck2_output, has_content_based_path = False))]

non_det_build = rule(
    impl = _non_det_build,
    attrs = {
        "buck2_output": attrs.string(),
    },
)

# Rule with no attrs that produces an output deterministically
def _trivial_build(ctx):
    return [DefaultInfo(default_output = ctx.actions.write("foo.txt", "abcd", has_content_based_path = False))]

trivial_build = rule(
    impl = _trivial_build,
    attrs = {},
)
