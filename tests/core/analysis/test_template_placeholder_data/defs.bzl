# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _args(v):
    return v

CombinerTset = transitive_set(
    args_projections = {
        "": _args,
    },
)

CombinerInfo = provider(fields = ["tset"])

def _combiner(ctx):
    output = ctx.actions.write("out", "", has_content_based_path = False)

    # the tset node's value will be our own output and the tset of ctx.attrs.extra_value if present
    # note that the normal way of combining tsets is by putting them in children and we want to be
    # able to explicitly test when a tset appears within the node value
    value = cmd_args(output)
    if ctx.attrs.extra_value != None:
        value.add(ctx.attrs.extra_value[CombinerInfo].tset.project_as_args(""))
    children = [c[CombinerInfo].tset for c in ctx.attrs.children]
    tset = ctx.actions.tset(CombinerTset, value = value, children = children)
    return [
        DefaultInfo(),
        CombinerInfo(
            tset = tset,
        ),
        TemplatePlaceholderInfo(
            keyed_variables = {
                "classpath_including_targets_with_no_output": tset.project_as_args(""),
            },
        ),
    ]

combiner = rule(
    impl = _combiner,
    attrs = {
        "children": attrs.list(attrs.dep(), default = []),
        "extra_value": attrs.option(attrs.dep(), default = None),
    },
)

def test(query_deps, expected_deps):
    query_labels = set([v.label for v in query_deps])
    expected_labels = set([v.label for v in expected_deps])
    expected_len = len(expected_labels)
    query_len = len(query_labels)
    if expected_len != query_len:
        return "different length ({} != {})".format(expected_len, query_len)
    for x in expected_labels:
        if not x in query_labels:
            return "missing expected item `{}`".format(x)

    for x in query_labels:
        if not x in expected_labels:
            return "additional item `{}`".format(x)

    return None

def _tester(ctx):
    res = test(ctx.attrs.query, ctx.attrs.expected)
    if res:
        fail("failed due to {}. expected `{}`, got `{}`".format(res, ctx.attrs.expected, ctx.attrs.query))

    return [DefaultInfo()]

tester = rule(
    impl = _tester,
    attrs = {
        "expected": attrs.list(attrs.dep()),
        "query": attrs.query(),
    },
)
