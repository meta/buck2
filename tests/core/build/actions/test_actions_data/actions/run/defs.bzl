# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _platform_args(args):
    if host_info().os.is_windows:
        return ["cmd.exe", "/c"] + args
    else:
        return args

def _run_command_impl(ctx):
    test = ctx.attrs.name
    if test == "runs_simple_script":
        declared = ctx.actions.declare_output(ctx.attrs.out, has_content_based_path = False)
        args = [
            ctx.attrs.script,
            declared.as_output(),
            "foo",
            ctx.attrs.other_src,
            "bar",
        ]
        ctx.actions.run(_platform_args(args), category = "simple_script")
        return [DefaultInfo(default_output = declared)]
    if test == "runs_simple_script_as_exe":
        declared = ctx.actions.declare_output(ctx.attrs.out, has_content_based_path = False)
        args = [
            declared.as_output(),
            "foo",
            ctx.attrs.other_src,
            "bar",
        ]
        exe = RunInfo(args = _platform_args([ctx.attrs.script]))
        ctx.actions.run(args, exe = exe, category = "simple_script")
        return [DefaultInfo(default_output = declared)]
    if test in ("runs_script_locally", "runs_script_locally_outputs_symlink"):
        declared = ctx.actions.declare_output(ctx.attrs.out, has_content_based_path = False)
        ctx.actions.run(_platform_args([ctx.attrs.script, declared.as_output()]), local_only = True, category = "local")
        return [DefaultInfo(default_output = declared)]
    elif test == "runs_simple_script_remote":
        declared = ctx.actions.declare_output(ctx.attrs.out, has_content_based_path = False)
        ctx.actions.run(_platform_args([ctx.attrs.script, declared.as_output()]), local_only = False, category = "remote")
        return [DefaultInfo(default_output = declared)]
    elif test == "rejects_zero_outputs":
        ctx.actions.run(_platform_args([ctx.attrs.script, "foo"]), category = "rejects_zero_outputs")
    elif test == "rejects_bad_args":

        def hide_type(x):
            return x

        ctx.actions.run(hide_type({}), category = "bad_args")
    else:
        fail("invalid test")
    return None

run_command = rule(
    impl = _run_command_impl,
    attrs = {
        "other_src": attrs.option(attrs.source(), default = None),
        "out": attrs.string(default = "out.txt"),
        "script": attrs.source(),
    },
)
