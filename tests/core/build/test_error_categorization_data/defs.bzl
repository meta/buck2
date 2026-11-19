# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _action_fail(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    ctx.actions.run(cmd_args("false", hidden = out.as_output()), category = "run")
    return [DefaultInfo(default_outputs = [out])]

action_fail = rule(
    impl = _action_fail,
    attrs = {},
)

def _action_missing_output(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(cmd_args("true", hidden = out.as_output()), category = "run")
    return [DefaultInfo(default_outputs = [out])]

missing_outputs = rule(
    impl = _action_missing_output,
    attrs = {},
)

def _bad_url(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    ctx.actions.download_file(out.as_output(), "doesnotexist640693486.com", sha1 = "1" * 40)
    return [DefaultInfo(default_output = out)]

bad_url = rule(
    impl = _bad_url,
    attrs = {},
)

def _run_action(ctx):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(cmd_args(["sh", "-c", 'echo > "$1"', "--", out.as_output()]), category = "run", local_only = ctx.attrs.local_only)
    return [DefaultInfo(default_outputs = [out])]

run_action = rule(
    impl = _run_action,
    attrs = {
        "local_only": attrs.bool(default = False),
    },
)

def _declared_cas_artifact(ctx):
    out = ctx.actions.cas_artifact(
        ctx.label.name,
        ctx.attrs.digest,
        ctx.attrs.use_case,
        expires_after_timestamp = 0,
        is_tree = ctx.attrs.is_tree,
    )
    return [DefaultInfo(default_output = out)]

declared_cas_artifact = rule(
    impl = _declared_cas_artifact,
    attrs = {
        "digest": attrs.string(),
        "is_tree": attrs.bool(default = False),
        "use_case": attrs.string(),
    },
)

def _slow(ctx):
    slow = ctx.actions.declare_output("slow", has_content_based_path = False)

    ctx.actions.run(
        ["fbpython", "-c", "import time, sys; time.sleep(60); sys.exit(1)", slow.as_output()],
        category = "slow_default_output",
    )

    return [DefaultInfo(slow)]

slow = rule(impl = _slow, attrs = {})
