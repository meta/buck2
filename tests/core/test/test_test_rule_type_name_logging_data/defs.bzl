# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _one(ctx):
    return [
        DefaultInfo(default_output = ctx.actions.write("out", "one", has_content_based_path = False)),
        ExternalRunnerTestInfo(
            command = ["fbpython", "-c", "import sys; sys.exit(0)"],
            type = "custom",
        ),
    ]

one = rule(
    impl = _one,
    attrs = {
        "deps": attrs.list(attrs.dep(), default = []),
    },
)

def _two(ctx):
    return [
        DefaultInfo(default_output = ctx.actions.write("out", "two", has_content_based_path = False)),
        ExternalRunnerTestInfo(
            command = ["fbpython", "-c", "import sys; sys.exit(0)"],
            type = "custom",
        ),
    ]

two = rule(
    impl = _two,
    attrs = {},
)

def _not_a_test(ctx):
    return [DefaultInfo(default_output = ctx.actions.write("out", "not_a_test", has_content_based_path = False))]

# A non-test rule. Mirrors macro-generated sibling targets (e.g. genrules)
# that users sometimes accidentally pass to `buck2 test`. Before this stack
# such invocations logged empty target_rule_type_names.
not_a_test = rule(
    impl = _not_a_test,
    attrs = {},
)
