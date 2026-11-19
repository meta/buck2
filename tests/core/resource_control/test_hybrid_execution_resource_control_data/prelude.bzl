# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

prefer_local = read_root_config("test", "prefer_local", "false").lower() == "true"

def _merge_outputs_impl(ctx) -> list[Provider]:
    outputs = []
    for i in range(1, ctx.attrs.num_actions):
        output = ctx.actions.declare_output("output{}.txt".format(i), has_content_based_path = False)
        outputs.append(output)

        sh_cmd = 'head -c 10 /dev/urandom > "$1"'
        if ctx.attrs.sleep > 0:
            sh_cmd = "sleep {} && {}".format(ctx.attrs.sleep, sh_cmd)

        ctx.actions.run(["sh", "-c", sh_cmd, "--", output.as_output()], category = "my_action{}".format(i), prefer_local = prefer_local)

    merged = ctx.actions.declare_output("output.txt", has_content_based_path = False)
    cmd = cmd_args("cat", outputs, ">", merged.as_output(), delimiter = " ")
    ctx.actions.run(["sh", "-c", cmd], category = "merge", prefer_local = prefer_local)

    return [
        DefaultInfo(merged),
    ]

merge_outputs = rule(
    impl = _merge_outputs_impl,
    attrs = {
        "num_actions": attrs.int(default = 100),
        "sleep": attrs.int(default = 0),
    },
)

def _execution_platform(ctx):
    return [
        DefaultInfo(),
        ExecutionPlatformInfo(
            label = ctx.label.raw_target(),
            configuration = ConfigurationInfo(constraints = {}, values = {}),
            executor_config = CommandExecutorConfig(
                local_enabled = True,
                remote_enabled = True,
                remote_execution_properties = {
                    "platform": "linux-remote-execution",
                },
                remote_execution_use_case = "buck2-default",
            ),
        ),
    ]

execution_platform = rule(
    impl = _execution_platform,
    attrs = {},
)

def _execution_platforms(ctx):
    return [
        DefaultInfo(),
        ExecutionPlatformRegistrationInfo(
            platforms = [p[ExecutionPlatformInfo] for p in ctx.attrs.platforms],
        ),
    ]

execution_platforms = rule(
    impl = _execution_platforms,
    attrs = {
        "platforms": attrs.list(attrs.dep(providers = [ExecutionPlatformInfo])),
    },
)

# Python interpreter overhead will add additional memory usage beyond the requested allocation.
SCRIPT = """
# args: size_mb, sleep_sec, output_path

import os
import sys
import time

PAGE = os.sysconf("SC_PAGE_SIZE")

def touch_every_page(buf: bytearray):
    # make sure we touch every page
    for i in range(0, len(buf), PAGE):
        buf[i] = 1

def main():
    if len(sys.argv) < 4:
        sys.exit(1)

    size_mb = int(sys.argv[1])
    sleep_sec = int(sys.argv[2])
    file_path = sys.argv[3]

    size_bytes = size_mb * 1024 * 1024
    # allocating size_mb MB of memory
    buf = bytearray(size_bytes)
    touch_every_page(buf)

    time.sleep(sleep_sec)

    del buf

    with open(file_path, "w") as f:
        f.write("done")

main()
"""

def _allocate_memory_impl(ctx) -> list[Provider]:
    outputs = []
    for i in range(0, ctx.attrs.num_actions):
        output = ctx.actions.declare_output("output{}.txt".format(i), has_content_based_path = False)
        each_action_memory_mb = ctx.attrs.each_action_memory_mb

        cmd = cmd_args(["fbpython", "-c", SCRIPT, each_action_memory_mb, str(ctx.attrs.sleep), output.as_output()])
        ctx.actions.run(cmd, category = "allocate_memory", identifier = str(i), prefer_local = prefer_local)

        outputs.append(output)

    merged = ctx.actions.declare_output("output.txt", has_content_based_path = False)
    cmd = cmd_args("cat", outputs, ">", merged.as_output(), delimiter = " ")
    ctx.actions.run(["sh", "-c", cmd], category = "merge", prefer_local = prefer_local)

    return [
        DefaultInfo(merged),
    ]

allocate_memory = rule(
    impl = _allocate_memory_impl,
    attrs = {
        "each_action_memory_mb": attrs.string(default = "10"),
        "num_actions": attrs.int(default = 10),
        "sleep": attrs.int(default = 0),
    },
)

_use_some_memory = read_root_config("use_some_memory", "path")

def _freeze_unfreeze_impl(ctx) -> list[Provider]:
    output0 = ctx.actions.declare_output("output0.txt", has_content_based_path = False)
    cmd0 = cmd_args(
        [_use_some_memory, "--allocate-count", "100", "--each-tick-allocate-memory", "2", "--tick-duration", "0.1", "--output", output0.as_output()]
    )

    # this action will be frozen
    ctx.actions.run(cmd0, category = "freeze_unfreeze", identifier = "action_to_be_frozen", prefer_local = prefer_local)

    output1 = ctx.actions.declare_output("output1.txt", has_content_based_path = False)

    # this action will not be frozen
    cmd1 = cmd_args([_use_some_memory, "--allocate-count", "40", "--each-tick-allocate-memory", "1", "--tick-duration", "0.1", "--output", output1.as_output()])
    ctx.actions.run(cmd1, category = "freeze_unfreeze", identifier = "small_action", prefer_local = prefer_local)

    final_output = ctx.actions.declare_output("final_output.txt", has_content_based_path = False)

    combine_output_script = """
    (cat "$1"; echo "========================"; cat "$2") > "$3"
    """
    ctx.actions.run(["sh", "-c", combine_output_script, "--", output0, output1, final_output.as_output()], category = "merge", prefer_local = prefer_local)

    return [
        DefaultInfo(final_output),
    ]

freeze_unfreeze = rule(
    impl = _freeze_unfreeze_impl,
    attrs = {},
)
