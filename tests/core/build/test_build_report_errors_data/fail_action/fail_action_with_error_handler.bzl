# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _error_handler_impl(ctx: ActionErrorCtx) -> list[ActionSubError]:
    indentation_error = regex(r"IndentationError")
    syntax_error = regex(r"SyntaxError")
    categories = []

    if indentation_error.match(ctx.stderr):
        categories.append(
            ctx.new_sub_error(
                category = "indentation",
                message = "Indentation error!",
            )
        )

    if syntax_error.match(ctx.stderr):
        categories.append(
            ctx.new_sub_error(
                category = "syntax",
                message = "Syntax error!",
                file = "not_really_the_right_file",
                lnum = 1,
            )
        )

    return categories

def _make_failing_action(ctx, src, name):
    out = ctx.actions.declare_output(src.short_path, has_content_based_path = False)
    ctx.actions.run(
        [
            "fbpython",
            src,
            out.as_output(),
        ],
        local_only = True,
        category = name,
        error_handler = _error_handler_impl,
    )

    return out

def _fail_one(ctx):
    return [DefaultInfo(default_outputs = [_make_failing_action(ctx, ctx.attrs.src, ctx.attrs.name)])]

def _fail_many(ctx):
    return [DefaultInfo(default_outputs = [_make_failing_action(ctx, src, ctx.attrs.name + str(i)) for (i, src) in enumerate(ctx.attrs.srcs)])]

def _make_failing_action_no_source(ctx, error_handler):
    out = ctx.actions.declare_output(ctx.attrs.name, has_content_based_path = False)

    # error handler is invoked but won't catch anything
    ctx.actions.run(
        cmd_args("false", hidden = out.as_output()),
        category = ctx.attrs.name,
        error_handler = error_handler,
    )

    return out

def _fail_one_no_op(ctx):
    # error handler is invoked but won't catch anything
    return [DefaultInfo(default_outputs = [_make_failing_action_no_source(ctx, _error_handler_impl)])]

def _error_handler_failed(ctx):
    def f(_ctx) -> list[ActionSubError]:
        fail("something went wrong")

    return [DefaultInfo(default_outputs = [_make_failing_action_no_source(ctx, f)])]

def _error_handler_wrong_return_type(ctx):
    def f(_ctx) -> int:
        return 1

    return [DefaultInfo(default_outputs = [_make_failing_action_no_source(ctx, f)])]

def _error_handler_produced_multiple_categories(ctx):
    def error_handler(ctx: ActionErrorCtx) -> list[ActionSubError]:
        categories = []

        categories.append(
            ctx.new_sub_error(
                category = "category1",
                message = "Message for category1",
            )
        )

        categories.append(
            ctx.new_sub_error(
                category = "category2",
                message = "Message for category2",
            )
        )

        return categories

    out = ctx.actions.declare_output(ctx.attrs.name, has_content_based_path = False)

    # error handler is invoked but won't catch anything
    ctx.actions.run(
        cmd_args("false", hidden = out.as_output()),
        category = ctx.attrs.name,
        error_handler = error_handler,
    )

    return [DefaultInfo(default_outputs = [out])]

def _fail_error_handler_with_output(ctx):
    out = ctx.actions.declare_output("output", has_content_based_path = ctx.attrs.use_content_based_path)

    def error_handler(ctx: ActionErrorCtx) -> list[ActionSubError]:
        categories = []
        file_str = ctx.output_artifacts[out].read_string()

        if "Compilation Error" in file_str:
            categories.append(
                ctx.new_sub_error(
                    category = "compilation",
                )
            )

        file_json = ctx.output_artifacts[out].read_json()

        if "ErrorCode123" in file_json["message"]:
            categories.append(
                ctx.new_sub_error(
                    category = "ErrorCode123",
                    message = "Try doing xyz",
                )
            )

        return categories

    ctx.actions.run(
        [
            "fbpython",
            ctx.attrs.src,
            out.as_output(),
        ],
        category = ctx.attrs.name,
        outputs_for_error_handler = [out.as_output()],
        error_handler = error_handler,
    )

    return [DefaultInfo(default_outputs = [out])]

fail_error_handler_with_output = rule(
    impl = _fail_error_handler_with_output,
    attrs = {
        "src": attrs.source(),
        "use_content_based_path": attrs.bool(default = read_config("test", "use_content_based_path", "") in ["true", "True"]),
    },
)

fail_one_with_error_handler = rule(
    impl = _fail_one,
    attrs = {
        "src": attrs.source(),
    },
)

fail_many_with_error_handler = rule(
    impl = _fail_many,
    attrs = {
        "srcs": attrs.list(attrs.source()),
    },
)

fail_one_with_error_handler_no_op = rule(
    impl = _fail_one_no_op,
    attrs = {},
)

error_handler_failed = rule(
    impl = _error_handler_failed,
    attrs = {},
)

error_handler_wrong_return_type = rule(
    impl = _error_handler_wrong_return_type,
    attrs = {},
)

error_handler_produced_multiple_categories = rule(
    impl = _error_handler_produced_multiple_categories,
    attrs = {},
)
