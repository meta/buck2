# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _chatty_rule_impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    ctx.actions.run(
        [
            "sh",
            "-c",
            'echo "hello from action stdout"; echo "hello from action stderr" >&2; touch "$1"',
            "--",
            out.as_output(),
        ],
        category = "chatty",
        local_only = True,
    )
    return [DefaultInfo(default_output = out)]

chatty_rule = rule(impl = _chatty_rule_impl, attrs = {})
