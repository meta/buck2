# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Simulates the typing_validation flow: a python_library-like rule that
# creates a ValidationSpec named "pyre" when typing_validation=True, using
# validation result JSON in the same format as
# type_check_result_to_validation.py output.

def _impl(ctx) -> list[Provider]:
    if ctx.attrs.typing and ctx.attrs.typing_validation:
        if ctx.attrs.has_type_errors:
            message = "bad_types.py:9:19 Argument `str` is not assignable to parameter `x` with type `int`"
            status = "failure"
        else:
            message = None
            status = "success"

        validation_result = ctx.actions.write_json(
            "type_check_validation.json",
            {
                "data": {
                    "message": message,
                    "status": status,
                },
                "version": 1,
            },
            pretty = True,
            has_content_based_path = False,
        )

        return [
            DefaultInfo(),
            ValidationInfo(
                validations = [
                    ValidationSpec(
                        name = "pyre",
                        validation_result = validation_result,
                    ),
                ],
            ),
        ]

    return [DefaultInfo()]

mock_python_library = rule(
    impl = _impl,
    attrs = {
        "has_type_errors": attrs.bool(default = False),
        "typing": attrs.bool(default = False),
        "typing_validation": attrs.bool(default = False),
    },
)
