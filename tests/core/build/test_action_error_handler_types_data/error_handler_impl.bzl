# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _assert_eq(a, b):
    if a != b:
        fail("Expected {} == {}".format(a, b))

def error_handler_impl(ctx: ActionErrorCtx) -> list[ActionSubError]:
    categories = []

    if "foo" in ctx.stdout:
        categories.append(
            ctx.new_sub_error(
                category = "foo_category",
                message = "foo message",
                file = "foo_file",
                lnum = 1,
            )
        )

    if "bar" in ctx.stderr:
        categories.append(
            ctx.new_sub_error(
                category = "bar_category",
                message = "bar message",
                file = "bar_file",
                lnum = 1,
            )
        )

    return categories

def _does_not_use_error_handler_impl(ctx: AnalysisContext):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["fbpython", "-c", "import sys\nsys.exit(1)"], hidden = out.as_output()),
        category = "test_failure",
        error_handler = error_handler_impl,
    )

    return [DefaultInfo(default_output = out)]

does_not_use_error_handler = rule(
    impl = _does_not_use_error_handler_impl,
    attrs = {},
)

def _error_handler_nonetype_impl(ctx: AnalysisContext):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["fbpython", "-c", "import sys; open(sys.argv[1], 'w').write('something')"], out.as_output()),
        category = "test_doesnt_fail",
        error_handler = None,
    )

    return [DefaultInfo(default_output = out)]

error_handler_nonetype_impl = rule(
    impl = _error_handler_nonetype_impl,
    attrs = {},
)

def error_handler_errorformat_impl(ctx: ActionErrorCtx) -> list[ActionSubError]:
    res = ctx.parse_with_errorformat(
        category = "test_failure0",
        error = ctx.stdout,
        errorformats = ["%f:%l: %m"],
    )
    _assert_eq(len(res), 1)
    _assert_eq(res[0].category, "test_failure0")
    _assert_eq(res[0].message, "expected `;`, found `}`")
    _assert_eq(res[0].file, "main.rs")
    _assert_eq(res[0].lnum, 10)
    _assert_eq(res[0].col, None)
    _assert_eq(res[0].end_lnum, None)
    _assert_eq(res[0].end_col, None)
    _assert_eq(res[0].error_type, None)
    _assert_eq(res[0].error_number, None)

    res[0].category = "test_failure"
    _assert_eq(res[0].category, "test_failure")
    _assert_eq(res[0].show_in_stderr, False)
    res[0].show_in_stderr = True

    new_sub_error = ctx.new_sub_error(
        category = "test_failure",
        message = "manually created sub error",
        show_in_stderr = False,
    )

    res.append(new_sub_error)
    return res

def _error_handler_with_errorformat(ctx: AnalysisContext):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["fbpython", "-c", "import sys\nprint('main.rs:10: expected `;`, found `}`')\nsys.exit(1)"], hidden = out.as_output()),
        category = "test_failure",
        error_handler = error_handler_errorformat_impl,
    )

    return [DefaultInfo(default_output = out)]

error_handler_with_errorformat = rule(
    impl = _error_handler_with_errorformat,
    attrs = {},
)
