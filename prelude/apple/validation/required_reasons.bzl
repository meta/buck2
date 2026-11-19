# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactTSet",  # @unused Used as a type
)
load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")

_AnalysisInput = record(
    argsfile = field(Artifact),
    identifier = field(int),
)

def get_required_reasons_validator_output(ctx, artifacts: ArtifactTSet) -> Artifact | None:
    if not ctx.attrs.validate_rrapi_usage:
        return None

    label_to_input_artifacts = _get_analysis_input_artifacts(ctx, artifacts)
    if not label_to_input_artifacts:
        return None

    required_reasons_tools = ctx.attrs._apple_toolchain[AppleToolchainInfo].required_reasons_tools
    label_to_analysis_artifacts = _analyze_artifacts(
        ctx,
        "required_reasons_usage",
        required_reasons_tools.analyzer,
        label_to_input_artifacts,
    )
    return _validate_analysis_artifacts(
        ctx,
        "required_reasons_usage",
        required_reasons_tools.validator,
        label_to_analysis_artifacts,
    )

def _get_analysis_input_artifacts(ctx, artifacts: ArtifactTSet) -> dict[Label, list[_AnalysisInput]]:
    underlying_tset = artifacts._tset
    if underlying_tset == None:
        return {}

    results = {}
    identifier = 0
    for infos in underlying_tset.traverse():
        for info in infos:
            argsfile = ctx.actions.write(
                "artifacts-{}.txt".format(identifier),
                info.artifacts,
                with_inputs = True,
                has_content_based_path = False,
            )
            results.setdefault(info.label, []).append(
                _AnalysisInput(argsfile = argsfile, identifier = identifier),
            )
            identifier += 1
    return results

def _analyze_artifacts(ctx, key: str, analysis_tool: RunInfo, label_to_artifacts: dict[Label, list[_AnalysisInput]]) -> dict[Label, list[Artifact]]:
    label_to_analysis = {}
    for label, inputs in label_to_artifacts.items():
        for input in inputs:
            output = ctx.actions.declare_output("{}_{}.json".format(key, input.identifier), has_content_based_path = False)
            ctx.actions.run(
                cmd_args([
                    analysis_tool,
                    "--artifacts",
                    cmd_args(input.argsfile, format = "@{}"),
                    "--output",
                    output.as_output(),
                ]),
                category = "{}_analysis".format(key),
                identifier = "{}_{}".format(ctx.attrs.name, input.identifier),
            )
            label_to_analysis.setdefault(label, []).append(output)

    return label_to_analysis

def _validate_analysis_artifacts(ctx, key: str, validator_tool: RunInfo, label_to_artifacts: dict[Label, list[Artifact]]) -> Artifact:
    input_json = ctx.actions.write_json(
        "{}_validator_args.json".format(key),
        label_to_artifacts,
        with_inputs = True,
        has_content_based_path = False,
    )

    output = ctx.actions.declare_output("{}.json".format(key), has_content_based_path = False)
    ctx.actions.run(
        cmd_args([
            validator_tool,
            "--analysis-json-path",
            input_json,
            "--output",
            output.as_output(),
        ]),
        category = "{}_validate".format(key),
        identifier = ctx.attrs.name,
    )
    return output
