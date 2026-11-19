# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _whistle(ctx):
    whistle = ctx.actions.declare_output("whistle", has_content_based_path = False)
    ctx.actions.run(
        [
            "ln",
            "-s",
            ".",
            whistle.as_output(),
        ],
        category = "test",
    )
    return [DefaultInfo(default_output = whistle)]

whistle = rule(impl = _whistle, attrs = {})

def _flute(ctx):
    flute = ctx.actions.declare_output("flute", has_content_based_path = False)
    ctx.actions.run(
        [
            "ln",
            "-s",
            "flute",
            flute.as_output(),
        ],
        category = "test",
    )
    return [DefaultInfo(default_output = flute)]

flute = rule(impl = _flute, attrs = {})
