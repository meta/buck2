# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _normal_impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)

    cmd = cmd_args(
        ctx.attrs.buck2_path,
        "build",
        "root//:trivial",
        "-c",
        "nested.buck2_path=" + ctx.attrs.buck2_path,
        "--out",
        out.as_output(),
    )
    ctx.actions.run(
        cmd,
        # Unset the user version. Actions will only sometimes do this, tests always will
        #
        # Unfortunately, no good way to ask for it to be unset, so we do this instead
        env = {"SANDCASTLE_ID": ""},
        local_only = True,
        category = "run",
    )
    return [DefaultInfo(default_output = out)]

def _trace_impl(ctx):
    trace_out = ctx.actions.declare_output("trace_out.txt", has_content_based_path = False)
    nested_out = ctx.actions.declare_output("nested_out.txt", has_content_based_path = False)

    script = ctx.actions.write(
        "script.py",
        """
import subprocess
import sys

buck_path = sys.argv[1]
subprocess.run([buck_path, "debug", "trace-io", "enable"])
    """,
        has_content_based_path = False,
    )
    ctx.actions.run(
        [
            "fbpython",
            script,
            ctx.attrs.buck2_path,
            trace_out.as_output(),
        ],
        local_only = True,
        category = "trace",
    )

    nested_cmd = cmd_args(
        ctx.attrs.buck2_path,
        "build",
        "root//:trivial",
        "-c",
        "nested.buck2_path=" + ctx.attrs.buck2_path,
        "--out",
        nested_out.as_output(),
        hidden = trace_out,
    )
    ctx.actions.run(
        nested_cmd,
        local_only = True,
        category = "run",
    )
    return [DefaultInfo(default_output = nested_out)]

def _no_buckd_impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)

    cmd = cmd_args(
        ctx.attrs.buck2_path,
        "build",
        "--no-buckd",
        "root//:trivial",
        "-c",
        "nested.buck2_path=" + ctx.attrs.buck2_path,
        "--out",
        out.as_output(),
    )
    ctx.actions.run(
        cmd,
        env = {"SANDCASTLE_ID": ""},
        local_only = True,
        category = "run",
    )
    return [DefaultInfo(default_output = out)]

normal_nested_invocation = rule(
    impl = _normal_impl,
    attrs = {
        "buck2_path": attrs.string(),
    },
)

no_buckd_nested_invocation = rule(
    impl = _no_buckd_impl,
    attrs = {
        "buck2_path": attrs.string(),
    },
)

trace_nested_invocation = rule(
    impl = _trace_impl,
    attrs = {
        "buck2_path": attrs.string(),
    },
)
