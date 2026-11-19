# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _hang(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        ["fbpython", "-c", 'import os, time; open(os.environ["TOUCH"], "w"); time.sleep(100)'],
        env = {"OUT": out.as_output(), "TOUCH": ctx.attrs.touch},
        category = "hang",
    )
    return [DefaultInfo(out)]

# Touch a file to signal, then hang.
hang = rule(attrs = {"touch": attrs.string()}, impl = _hang)

def _pass(ctx):
    out = ctx.actions.write("out", "", has_content_based_path = False)
    return [DefaultInfo(out)]

pass_ = rule(attrs = {}, impl = _pass)

def _kill(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        ["fbpython", "-c", 'import os, signal; os.kill(int(os.environ["PID"]), signal.SIGKILL)'],
        env = {"OUT": out.as_output(), "PID": ctx.attrs.pid},
        category = "kill",
    )
    return [DefaultInfo(out)]

kill = rule(attrs = {"pid": attrs.string()}, impl = _kill)

def _fail(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            "sh",
            "-c",
            'echo "Hi from stderr!" >&2 && false',
            hidden = out.as_output(),
        ),
        category = "fail",
    )
    return [DefaultInfo(out)]

fail = rule(attrs = {}, impl = _fail)

def _one(ctx):
    return [DefaultInfo(default_output = ctx.actions.write("out", "one", has_content_based_path = False))]

one = rule(
    impl = _one,
    attrs = {},
)

def _two(ctx):
    return [DefaultInfo(default_output = ctx.actions.write("out", "two", has_content_based_path = False))]

two = rule(
    impl = _two,
    attrs = {},
)

def _sleep(ctx):
    # sleep for 5 seconds to ensure all hg commands are finished
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["fbpython", "-c", "import sys, time; time.sleep(5); open(sys.argv[1], 'w').write('something')"], out.as_output()),
        category = "sleep",
    )
    return [DefaultInfo(out)]

sleep = rule(
    impl = _sleep,
    attrs = {},
)

def _run(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["fbpython", "-c", "import sys; open(sys.argv[1], 'w').write('something')"], out.as_output()),
        category = "sleep",
    )
    return [DefaultInfo(out)]

run = rule(
    impl = _run,
    attrs = {},
)
