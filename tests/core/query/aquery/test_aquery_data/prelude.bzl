# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _test(ctx: AnalysisContext):
    dep = ctx.actions.write("dep", "", has_content_based_path = False)
    default = ctx.actions.copy_file("default", dep, has_content_based_path = False)
    other = ctx.actions.write("other", "", has_content_based_path = False)
    other_default = ctx.actions.copy_file("other_default", dep, has_content_based_path = False)

    sub_default = ctx.actions.write("sub_default", "", has_content_based_path = False)
    sub_other = ctx.actions.write("sub_other", "", has_content_based_path = False)

    # Unused
    ctx.actions.write("unused", "", has_content_based_path = False)

    return [
        DefaultInfo(
            default_outputs = [default, other_default],
            other_outputs = [other],
            sub_targets = {
                "sub": [
                    DefaultInfo(
                        default_outputs = [sub_default],
                        other_outputs = [sub_other],
                    ),
                ],
            },
        )
    ]

test = rule(impl = _test, attrs = {})
