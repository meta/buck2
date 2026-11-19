# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _write_rel_action(ctx: AnalysisContext) -> list[Provider]:
    a = ctx.actions.declare_output("uuuuuu", has_content_based_path = False)
    b = a.project("rel")
    ctx.actions.write(b.as_output(), "ccoonntteenntt")
    return [DefaultInfo(default_output = a)]

write_rel_action = rule(
    impl = _write_rel_action,
    attrs = {},
)

def _run_rel_action(ctx: AnalysisContext) -> list[Provider]:
    a = ctx.actions.declare_output("uuuuuu", has_content_based_path = False)
    b = a.project("rel")
    ctx.actions.run(
        cmd_args(
            "fbpython",
            "-c",
            """
import sys
import os

f = sys.argv[1]

# Here we also assert that the directory does not exist yet.
os.mkdir(os.path.dirname(f))

with open(f, "w") as f:
    f.write("hello")
""",
            b.as_output(),
        ),
        category = "ignore",
    )
    return [DefaultInfo(default_output = a)]

run_rel_action = rule(
    impl = _run_rel_action,
    attrs = {},
)
