# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# @nolint

def _tr(platform, refs, attrs):
    _ignore = platform
    constraint_value = getattr(refs, attrs.use_constraint)[ConstraintValueInfo]
    return PlatformInfo(
        label = "pppppppppppppppppppppppppppppppppppppppppppppppppppppppppppp",
        configuration = ConfigurationInfo(
            constraints = {
                constraint_value.setting.label: constraint_value,
            },
            values = {},
        ),
    )

tr = transition(
    impl = _tr,
    refs = {
        "a": "//recursive_transition:a",
        "b": "//recursive_transition:b",
        "c": "//recursive_transition:c",
    },
    attrs = [
        "use_constraint",
    ],
)

def _ooo(_ctx):
    return [DefaultInfo()]

ooo = rule(
    impl = _ooo,
    attrs = {
        "use_constraint": attrs.string(),
        "deps": attrs.list(attrs.dep()),
    },
    cfg = tr,
)
