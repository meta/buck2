# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _make_failing_action(ctx, name):
    out = ctx.actions.declare_output(name, has_content_based_path = False)
    ctx.actions.run(cmd_args("false", hidden = out.as_output()), category = "fail" + name)
    return out

def _action_alias(ctx, actual, name):
    out = ctx.actions.declare_output(name, has_content_based_path = False)
    ctx.actions.run(
        cmd_args("doesntmatter", hidden = [out.as_output(), actual]),
        category = "alias" + name,
    )
    return out

def _fail_one_impl(ctx):
    return [DefaultInfo(default_outputs = [_make_failing_action(ctx, "name")])]

def _fail_two_impl(ctx):
    return [
        DefaultInfo(
            default_outputs = [
                _make_failing_action(ctx, "name_a"),
                _make_failing_action(ctx, "name_b"),
            ]
        )
    ]

def _fail_shared_dep_impl(ctx):
    fail = _make_failing_action(ctx, "fail")
    a = _action_alias(ctx, fail, "a")
    b = _action_alias(ctx, fail, "b")
    return [DefaultInfo(default_outputs = [a, b])]

def _action_alias_impl(ctx):
    actual = ctx.attrs.actual[DefaultInfo].default_outputs[0]
    aliased = _action_alias(ctx, actual, "aliased")
    return [DefaultInfo(default_outputs = [aliased])]

def _fail_two_deps_impl(ctx):
    first = _make_failing_action(ctx, "first")
    second = _make_failing_action(ctx, "second")
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args("doesntmatter", hidden = [first, second, out.as_output()]),
        category = "out",
    )
    return [DefaultInfo(default_outputs = [out])]

def _fail_script_impl(ctx):
    out = ctx.actions.declare_output("fail_script", has_content_based_path = False)
    ctx.actions.run(
        [
            "fbpython",
            "-c",
            "import sys; print('Some random stdout', file=sys.stdout); print('Some random stderr', file=sys.stderr); sys.exit(1)",
            out.as_output(),
        ],
        category = "fail_script",
    )

    return [DefaultInfo(default_outputs = [out])]

fail_one = rule(
    impl = _fail_one_impl,
    attrs = {},
)

fail_two = rule(
    impl = _fail_two_impl,
    attrs = {},
)

fail_shared_dep = rule(
    impl = _fail_shared_dep_impl,
    attrs = {},
)

action_alias = rule(
    impl = _action_alias_impl,
    attrs = {"actual": attrs.dep()},
)

fail_two_deps = rule(
    impl = _fail_two_deps_impl,
    attrs = {},
)

fail_script = rule(
    impl = _fail_script_impl,
    attrs = {},
)
