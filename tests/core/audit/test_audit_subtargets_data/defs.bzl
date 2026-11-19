# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _no_subtargets_impl(_ctx):
    return [DefaultInfo()]

no_subtargets = rule(
    impl = _no_subtargets_impl,
    attrs = {},
)

def _subtargets_impl(_ctx):
    return [
        DefaultInfo(
            sub_targets = {
                "bar": [DefaultInfo()],
                "baz": [DefaultInfo()],
            },
        )
    ]

subtargets = rule(
    impl = _subtargets_impl,
    attrs = {},
)

def _nested_subtargets_impl(_ctx):
    return [
        DefaultInfo(
            sub_targets = {
                "sub1": [
                    DefaultInfo(
                        sub_targets = {
                            "sub2": [DefaultInfo()],
                            "sub3": [DefaultInfo()],
                        },
                    )
                ],
                "sub4": [DefaultInfo()],
            },
        )
    ]

nested_subtargets = rule(
    impl = _nested_subtargets_impl,
    attrs = {},
)

def _deeply_nested_subtargets_impl(_ctx):
    return [
        DefaultInfo(
            sub_targets = {
                "sub1": [DefaultInfo()],
                "sub2": [
                    DefaultInfo(
                        sub_targets = {
                            "sub3": [
                                DefaultInfo(
                                    sub_targets = {"sub4": [DefaultInfo()]},
                                )
                            ],
                            "sub5": [DefaultInfo()],
                        },
                    )
                ],
            },
        )
    ]

deeply_nested_subtargets = rule(
    impl = _deeply_nested_subtargets_impl,
    attrs = {},
)
