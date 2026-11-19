# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//linking:link_info.bzl",
    "ExtraLinkerOutputs",
)
load("@prelude//utils:lazy.bzl", "lazy")

def merge_linker_args_for_platform(merge_linker_args: dict[str, typing.Any], platform: str | None) -> dict[str, typing.Any]:
    """The `native_library_merge_linker_args` entries for one platform.

    A key `<platform>:<soname>` (or `<platform>:<synthesized>`) applies only to
    that platform's libraries: its args follow the unscoped entry's, if any.
    Keys scoped to other platforms are dropped; unscoped keys apply to every
    platform. Without scoped keys the dict is returned unchanged.
    """
    if not lazy.is_any(lambda key: ":" in key, merge_linker_args):
        return merge_linker_args
    result = {}
    for key, args in merge_linker_args.items():
        if ":" not in key:
            result[key] = list(args)
    for key, args in merge_linker_args.items():
        if ":" not in key:
            continue
        scope, name = key.split(":", 1)
        if scope == platform:
            result[name] = result.get(name, []) + list(args)
    return result

def merge_extra_linker_args(arg_dicts: list[dict[str, typing.Any]]) -> dict[str, typing.Any]:
    """Merge multiple extra linker output factory dicts.

    This function combines multiple extra linker output configurations into a single
    configuration. It handles both the outputs factory (which produces artifacts and
    providers) and the flags factory (which produces linker flags).

    Args:
        arg_dicts: A list of dicts, each potentially containing:
            - "extra_linker_outputs_factory": A function (ctx) -> ExtraLinkerOutputs
            - "extra_linker_outputs_flags_factory": A function (ctx, outputs) -> list

    Returns:
        A merged dict with combined factory functions, or an empty dict if no
        non-empty dicts were provided.
    """
    non_empty = [d for d in arg_dicts if d]
    if len(non_empty) == 0:
        return {}
    if len(non_empty) == 1:
        return non_empty[0]

    def combined_outputs_factory(ctx: AnalysisContext) -> ExtraLinkerOutputs:
        artifacts = {}
        providers = {}
        for arg_dict in non_empty:
            if "extra_linker_outputs_factory" in arg_dict:
                result = arg_dict["extra_linker_outputs_factory"](ctx)
                artifacts |= result.artifacts
                providers |= result.providers
        return ExtraLinkerOutputs(artifacts = artifacts, providers = providers)

    def combined_flags_factory(ctx: AnalysisContext, outputs: dict[str, Artifact]) -> list:
        flags = []
        for arg_dict in non_empty:
            if "extra_linker_outputs_flags_factory" in arg_dict:
                flags += arg_dict["extra_linker_outputs_flags_factory"](ctx, outputs)
        return flags

    return {
        "extra_linker_outputs_factory": combined_outputs_factory,
        "extra_linker_outputs_flags_factory": combined_flags_factory,
    }

def package_validators_decorator(ctx: AnalysisContext, build_func, extension: str):
    package_validators = ctx.attrs.package_validators
    if not package_validators:
        return build_func

    def wrapped_build_func(**build_kwargs):
        final_filename = build_kwargs["output_filename"]
        build_kwargs["output_filename"] = final_filename + "_pre_validation"
        output = build_func(**build_kwargs)

        validation_outputs = _get_package_validation_outputs(
            actions = ctx.actions,
            package_validators = package_validators,
            package_output = output,
        )
        final_output = ctx.actions.declare_output(
            final_filename + extension,
            has_content_based_path = False,
        )
        ctx.actions.run(
            cmd_args(
                [
                    "cp",
                    output,
                    final_output.as_output(),
                ],
                hidden = validation_outputs,
            ),
            category = "package_validators",
            identifier = "symlink_original_output",
        )

        return final_output

    return wrapped_build_func

def _get_package_validation_outputs(actions: AnalysisActions, package_validators: list, package_output: Artifact) -> list[Artifact]:
    if not package_validators:
        return []

    outputs = []
    for idx, validator in enumerate(package_validators):
        validator, validator_args = validator

        output = actions.declare_output(validator.label.name + "_package_validator_{}".format(idx), has_content_based_path = False)
        outputs.append(output)

        actions.run(
            cmd_args([
                validator[RunInfo],
                "--package",
                package_output,
                "--output-path",
                output.as_output(),
                validator_args,
            ]),
            category = "package_validator",
            identifier = str(idx),
        )

    return outputs

# A wrapper around AnalysisContext that makes it easier to define subtargets without needing to pass information
# for them all the way back to the outermost analysis impl.
EnhancementContext = record(
    ctx = AnalysisContext,
    actions = AnalysisActions,
    attrs = typing.Any,
    label = Label,
    # methods
    debug_output = typing.Callable,
    get_sub_targets = typing.Callable,
)

def create_enhancement_context(ctx: AnalysisContext) -> EnhancementContext:
    extra_sub_targets = {}

    def debug_output(name: str, output: Artifact, other_outputs = [], sub_targets: dict[str, typing.Any] = {}):
        """Adds a subtarget to expose debugging outputs."""
        extra_sub_targets[name] = [DefaultInfo(default_outputs = [output], other_outputs = other_outputs, sub_targets = sub_targets)]

    def get_sub_targets():
        return extra_sub_targets

    return EnhancementContext(
        ctx = ctx,
        actions = ctx.actions,
        attrs = ctx.attrs,
        label = ctx.label,
        debug_output = debug_output,
        get_sub_targets = get_sub_targets,
    )
