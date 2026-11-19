# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _builds(ctx: AnalysisContext):
    # Create locally
    stage0 = ctx.actions.declare_output("stage0", has_content_based_path = False)
    ctx.actions.run(
        ["sh", "-c", 'head -c 10 /dev/urandom > "$1"', "--", stage0.as_output()],
        category = "stage0",
        local_only = True,
    )

    # Use on RE
    stage1 = ctx.actions.declare_output("stage1", has_content_based_path = False)
    ctx.actions.run(["sh", "-c", 'cat "$1" "$1" > "$2"', "--", stage0, stage1.as_output()], category = "stage1")

    # Reuse on RE
    stage2 = ctx.actions.declare_output("stage2", has_content_based_path = False)
    ctx.actions.run(["sh", "-c", 'cat "$1" "$1" > "$2"', "--", stage1, stage2.as_output()], category = "stage2")

    # Reuse locally
    stage3 = ctx.actions.declare_output("stage3", has_content_based_path = False)
    ctx.actions.run(
        ["sh", "-c", 'cat "$1" "$1" > "$2"', "--", stage2, stage3.as_output()],
        category = "stage3",
        local_only = True,
    )

    # Verify
    stage4 = ctx.actions.declare_output("stage4", has_content_based_path = False)
    ctx.actions.run(
        [
            "sh",
            "-c",
            'cat "$1" "$1" "$1" "$1" "$1" "$1" "$1" "$1" > "$3" && diff "$2" "$3"',
            "--",
            stage0,
            stage3,
            stage4.as_output(),
        ],
        category = "stage4",
    )

    return [DefaultInfo(stage4)]

builds = rule(attrs = {}, impl = _builds)
