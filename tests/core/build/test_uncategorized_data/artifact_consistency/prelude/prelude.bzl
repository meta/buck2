# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _gen_files(ctx):
    for i in range(0, ctx.attrs.action_index):
        ctx.actions.write("action-{}".format(i), "nonsense data", has_content_based_path = False)

    output = ctx.actions.declare_output("out_dir", dir = True, has_content_based_path = False)
    subouts = []
    subtargets = {}
    for f in ctx.attrs.files:
        out = output.project(f)
        subouts.append(out.as_output())
        subtargets[f] = [DefaultInfo(default_outputs = [out])]
    args = cmd_args(
        ["fbpython", ctx.attrs.script, output.as_output()] + ctx.attrs.files,
        hidden = subouts,
    )
    ctx.actions.run(args, category = "gen")
    return [DefaultInfo(sub_targets = subtargets)]

gen_files = rule(
    impl = _gen_files,
    attrs = {
        "action_index": attrs.int(),
        "files": attrs.list(attrs.string()),
        "script": attrs.source(),
    },
)
