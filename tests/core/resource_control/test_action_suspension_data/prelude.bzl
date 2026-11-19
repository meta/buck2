# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

_use_some_memory = read_root_config("use_some_memory", "path")

_start_marker_files = read_root_config("start_marker_files", "path")

def _memory_allocating_actions_impl(ctx: AnalysisContext) -> list[Provider]:
    all_outputs = []
    for i in range(ctx.attrs.width):
        last_output = cmd_args()
        for j in range(ctx.attrs.depth):
            ident = "action_{}_{}".format(i, j)
            output = ctx.actions.declare_output(ident, has_content_based_path = False)
            all_outputs.append(output)

            if _start_marker_files:
                start_marker_args = cmd_args(
                    "--start-marker",
                    _start_marker_files + "/" + ident,
                )
            else:
                start_marker_args = cmd_args()
            cmd = cmd_args(
                _use_some_memory,
                "--allocate-count",
                str(ctx.attrs.ticks),
                "--each-tick-allocate-memory",
                str(ctx.attrs.memory_per_tick_mb),
                "--tick-duration",
                str(ctx.attrs.tick_duration_ms / 1000.0),
                "--pre-exit-sleep-duration",
                str(ctx.attrs.sleep_ms / 1000.0),
                "--output",
                output.as_output(),
                start_marker_args,
                hidden = last_output,
            )
            ctx.actions.run(cmd, category = "memory_allocating_actions", identifier = ident)
            last_output = output

    # Have to do this because we don't show more than one default output for `--show-output`
    output_paths = ctx.actions.write("output_paths", all_outputs, has_content_based_path = False)

    return [DefaultInfo(default_output = output_paths, other_outputs = all_outputs)]

memory_allocating_actions = rule(
    impl = _memory_allocating_actions_impl,
    attrs = {
        "depth": attrs.int(),
        "memory_per_tick_mb": attrs.int(),
        "sleep_ms": attrs.int(),
        "tick_duration_ms": attrs.int(),
        "ticks": attrs.int(),
        "width": attrs.int(),
    },
)
