# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

MyInfo = provider(fields = {"hgfd": int})

def _produce_dynamic_value_impl(actions: AnalysisActions) -> list[Provider]:
    _ignore = (actions,)  # buildifier: disable=unused-variable
    return [
        MyInfo(hgfd = 123),
    ]

_produce_dynamic_value = dynamic_actions(
    impl = _produce_dynamic_value_impl,
    attrs = {},
)

def _consume_dynamic_value_impl(actions, out: OutputArtifact, v: ResolvedDynamicValue) -> list[Provider]:
    value = v.providers[MyInfo].hgfd
    actions.write(out, "<<<{}>>>".format(value))
    return []

_consume_dynamic_value = dynamic_actions(
    impl = _consume_dynamic_value_impl,
    attrs = {
        "out": dynattrs.output(),
        "v": dynattrs.dynamic_value(),
    },
)

def _test_rule(ctx):
    v = ctx.actions.dynamic_output_new(_produce_dynamic_value())

    out = ctx.actions.declare_output("poiuy", has_content_based_path = False)
    ctx.actions.dynamic_output_new(
        _consume_dynamic_value(
            v = v,
            out = out.as_output(),
        )
    )

    return [DefaultInfo(default_output = out)]

test_rule = rule(
    impl = _test_rule,
    attrs = {},
)
