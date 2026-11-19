# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

target = rule(
    impl = lambda ctx: [
        DefaultInfo(default_output = ctx.actions.write("out", ctx.attrs.arg or "", allow_args = True, has_content_based_path = False)[0]),
        RunInfo(),
    ],
    attrs = {"arg": attrs.option(attrs.arg(), default = None)},
)

def _duplicate_queries_impl(ctx):
    for query in [ctx.attrs.first, ctx.attrs.second]:
        if [dep.label for dep in query] != [ctx.attrs.expected.label]:
            fail("Unexpected query result: {}".format(query))
    return [DefaultInfo(default_output = ctx.actions.write("out", ctx.attrs.arg, allow_args = True, has_content_based_path = False)[0])]

duplicate_queries = rule(
    impl = _duplicate_queries_impl,
    attrs = {
        "arg": attrs.arg(),
        "expected": attrs.dep(),
        "first": attrs.query(),
        "second": attrs.query(),
    },
)

def defs():
    # Targets we want to run deps queries from
    target(
        name = "deps",
        arg = "$(query_targets deps(:foo))",
    )
    target(
        name = "deps1",
        arg = "$(query_targets deps(:foo, 1))",
    )
    target(
        name = "target_deps",
        arg = "$(query_targets deps(:foo, 100000, target_deps()))",
    )
    duplicate_queries(
        name = "duplicate_queries",
        arg = "$(query_targets deps(:bar))|$(query_outputs deps(:bar))|$(query_targets deps(:bar))|$(query_outputs deps(:bar))",
        expected = ":bar",
        first = "deps(:bar)",
        second = "deps(:bar)",
    )

    # Targets that are deps of targets we want to run deps queries from.
    target(
        name = "foo",
        arg = "$(location :bar) $(exe :baz)",
    )
    target(name = "bar")
    target(name = "baz", arg = "$(location :qux)")
    target(name = "qux")
