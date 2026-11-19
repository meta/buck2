# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _dummy_impl(ctx: AnalysisContext) -> list[Provider]:
    output = ctx.actions.write("{}.txt".format(ctx.label.name), "{}\n{}".format(ctx.attrs.os, ctx.attrs.cpu), has_content_based_path = False)
    return [DefaultInfo(default_outputs = [output])]

def _impl(_ctx: AnalysisContext) -> list[Provider]:
    return [DefaultInfo()]

dummy = rule(
    impl = _dummy_impl,
    attrs = {
        "cpu": attrs.string(),
        "os": attrs.string(),
    },
)

test_universe = rule(
    impl = _impl,
    attrs = {
        "deps": attrs.list(attrs.dep()),
    },
)
