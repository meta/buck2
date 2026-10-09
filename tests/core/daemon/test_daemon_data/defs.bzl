# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _long_running_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        ["fbpython", "-c", "import time, sys; time.sleep(999999); open(sys.argv[1],'w')", out.as_output()],
        category = "test",
        identifier = "id",
    )

    return [DefaultInfo(out)]

long_running = rule(
    impl = _long_running_impl,
    attrs = {},
)

def _generation_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        [
            "fbpython",
            "-c",
            "import os, pathlib, sys, time; out, started, release, value = sys.argv[1:]; previous = pathlib.Path(started).with_name('old.started.pid');\nif value != 'old' and previous.exists():\n try: os.kill(int(previous.read_text()), 0)\n except ProcessLookupError: pass\n else: raise RuntimeError('previous tenant action is still running')\npathlib.Path(started + '.pid').write_text(str(os.getpid())); pathlib.Path(started).write_text(str(pathlib.Path(out).resolve()));\nwhile not pathlib.Path(release).exists():\n pathlib.Path(out).write_text(value); time.sleep(0.05)\npathlib.Path(out).write_text(value)",
            out.as_output(),
            ctx.attrs.started,
            ctx.attrs.release,
            ctx.attrs.value,
        ],
        category = "generation",
        local_only = True,
    )
    return [DefaultInfo(out)]

generation = rule(
    impl = _generation_impl,
    attrs = {"release": attrs.string(), "started": attrs.string(), "value": attrs.string()},
)

def _tenant_value_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.write("value.txt", ctx.attrs.value)
    return [DefaultInfo(default_output = out)]

tenant_value = rule(
    impl = _tenant_value_impl,
    attrs = {"value": attrs.string()},
)

def _pwd_run_impl(_ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        RunInfo(args = cmd_args("fbpython", "-c", "import os; print(os.getcwd())")),
    ]

# Prints the working directory a `buck2 run` target starts in.
pwd_run = rule(
    impl = _pwd_run_impl,
    attrs = {},
)
