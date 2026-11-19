# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _write_json_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.write_json("out.json", ctx.attrs.content, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

write_json = rule(
    impl = _write_json_impl,
    attrs = {
        "content": attrs.string(default = "text"),
    },
)
