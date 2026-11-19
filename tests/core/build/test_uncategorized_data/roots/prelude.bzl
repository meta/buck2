# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _test(ctx: AnalysisContext):
    fixture = ctx.actions.write("fixture", "", has_content_based_path = False)

    out = ctx.actions.write_json(
        "out.json",
        {
            "cell_relative_to_fixture": cmd_args(ctx.label.cell_root, delimiter = "", relative_to = fixture),
            "fixture_relative_to_cell": cmd_args(fixture, delimiter = "", relative_to = ctx.label.cell_root),
            "fixture_relative_to_project": cmd_args(fixture, delimiter = "", relative_to = ctx.label.project_root),
            "project_relative_to_fixture": cmd_args(ctx.label.project_root, delimiter = "", relative_to = fixture),
        },
        has_content_based_path = False,
    )

    return [DefaultInfo(out, other_outputs = [fixture])]

test = rule(impl = _test, attrs = {})
