# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

MetadataInfo = provider(fields = {"count": int, "name": str})

DynamicValueInfo = provider(fields = {"value": DynamicValue})

def _produce_metadata_impl(actions: AnalysisActions, name: str) -> list[Provider]:
    _ignore = (actions,)  # buildifier: disable=unused-variable
    return [MetadataInfo(name = name, count = 42)]

_produce_metadata = dynamic_actions(
    impl = _produce_metadata_impl,
    attrs = {
        "name": dynattrs.value(str),
    },
)

def _fail_in_callback_impl(actions: AnalysisActions) -> list[Provider]:
    _ignore = (actions,)  # buildifier: disable=unused-variable
    fail("producer callback failed")

_fail_in_callback = dynamic_actions(
    impl = _fail_in_callback_impl,
    attrs = {},
)

def _read_failed_artifact_impl(actions: AnalysisActions, src: ArtifactValue) -> list[Provider]:
    _ignore = (actions, src)  # buildifier: disable=unused-variable
    fail("producer callback should not run")

_read_failed_artifact = dynamic_actions(
    impl = _read_failed_artifact_impl,
    attrs = {
        "src": dynattrs.artifact_value(),
    },
)

def _metadata_impl(ctx):
    value = ctx.actions.dynamic_output_new(_produce_metadata(name = ctx.label.name))
    return [DefaultInfo(), DynamicValueInfo(value = value)]

metadata = rule(impl = _metadata_impl, attrs = {})

def _anon_metadata_impl(ctx):
    value = ctx.actions.dynamic_output_new(_produce_metadata(name = ctx.attrs.metadata_name))
    return [DefaultInfo(), DynamicValueInfo(value = value)]

anon_metadata = anon_rule(
    impl = _anon_metadata_impl,
    attrs = {"metadata_name": attrs.string()},
    artifact_promise_mappings = {},
)

def _callback_fails_impl(ctx):
    value = ctx.actions.dynamic_output_new(_fail_in_callback())
    return [DefaultInfo(), DynamicValueInfo(value = value)]

callback_fails = rule(impl = _callback_fails_impl, attrs = {})

def _artifact_fails_impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["sh", "-c", "printf 'upstream stdout'; printf 'upstream stderr' >&2; exit 1", "--", out.as_output()]),
        category = "failing_upstream",
    )
    value = ctx.actions.dynamic_output_new(_read_failed_artifact(src = out))
    return [DefaultInfo(), DynamicValueInfo(value = value)]

artifact_fails = rule(impl = _artifact_fails_impl, attrs = {})
