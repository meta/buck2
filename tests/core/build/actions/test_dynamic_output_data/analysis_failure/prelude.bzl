# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _analysis_failure_f_impl(actions: AnalysisActions, input: ArtifactValue, output: OutputArtifact):
    _ignore = (actions, input, output)  # buildifier: disable=unused-variable
    fail("Analysis failed: this is a test failure message")

_analysis_failure_f = dynamic_actions(
    impl = _analysis_failure_f_impl,
    attrs = {
        "input": dynattrs.artifact_value(),
        "output": dynattrs.output(),
    },
)

def _analysis_failure_impl(ctx: AnalysisContext) -> list[Provider]:
    input = ctx.actions.write("input", "test", has_content_based_path = False)
    output = ctx.actions.declare_output("output", has_content_based_path = False)

    ctx.actions.dynamic_output_new(
        _analysis_failure_f(
            input = input,
            output = output.as_output(),
        )
    )
    return [DefaultInfo(default_output = output)]

analysis_failure_test = rule(impl = _analysis_failure_impl, attrs = {})
