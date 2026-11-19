# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

_ATTRS_VALIDATORS_NAME = "attrs_validators"

AttrsValidatorsInfo = provider(
    fields = {
        "func": typing.Callable[[AnalysisActions, Label, struct], dict[str, Artifact]],
    },
)

def get_attrs_validation_specs(ctx: AnalysisContext) -> list[ValidationSpec]:
    validators = getattr(ctx.attrs, _ATTRS_VALIDATORS_NAME, [])
    if not validators:
        return []

    specs = []
    for validator in validators:
        for name, output in validator[AttrsValidatorsInfo].func(ctx.actions, ctx.label, ctx.attrs).items():
            specs.append(ValidationSpec(name = name, validation_result = output))

    return specs

def _attrs_validators_arg():
    return {
        _ATTRS_VALIDATORS_NAME: attrs.option(
            attrs.list(attrs.dep(providers = [AttrsValidatorsInfo])),
            default = None,
        ),
    }

def _validation_specs_arg():
    return {
        "validation_specs": attrs.dict(
            attrs.string(),
            attrs.source(
                doc = """
                An artifact pointing to a JSON file that will be used in ValidationSpec.
                {
                  "version": 1,
                  "data": {
                    "message": "What goes in stderr",
                    "status": "success" | "failure",
                  }
                } 
            """
            ),
            default = {},
        ),
    }

validation_common = struct(
    attrs_validators_arg = _attrs_validators_arg,
    validation_specs_arg = _validation_specs_arg,
)
