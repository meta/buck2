# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

_WRITE_DIR = """
import os
import sys

root = sys.argv[1]
os.makedirs(root, exist_ok=True)
for entry in sys.argv[2:]:
    name, _, content = entry.partition("=")
    with open(os.path.join(root, name), "w") as f:
        f.write(content)
"""

_CAT = """
import sys

with open(sys.argv[1], "w") as out:
    out.write("|".join(open(p).read() for p in sys.argv[2:]))
"""

def _produce_impl(ctx):
    out = ctx.actions.declare_output("out", dir = True, has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            ["fbpython", "-c", _WRITE_DIR, out.as_output()],
            ["{}={}".format(name, content) for name, content in ctx.attrs.contents.items()],
        ),
        category = "produce",
    )
    return [DefaultInfo(default_output = out)]

produce = rule(
    impl = _produce_impl,
    attrs = {
        "contents": attrs.dict(attrs.string(), attrs.string()),
    },
)

def _consume_impl(ctx):
    produced = ctx.attrs.dep[DefaultInfo].default_outputs[0]
    inputs = [produced.project(path) for path in ctx.attrs.projections]
    inputs += [cmd_args(produced, format = "{}/" + entry) for entry in ctx.attrs.whole_dir_entries]

    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["fbpython", "-c", _CAT, out.as_output()], inputs),
        category = "consume",
        # Force the inputs to be materialized on disk rather than read from
        # wherever the action would otherwise have run.
        local_only = True,
    )
    return [DefaultInfo(default_output = out)]

# Reads its inputs and writes them out `|`-separated. `projections` names
# subpaths consumed as projections of `dep`'s directory artifact;
# `whole_dir_entries` names subpaths reached by consuming the whole directory
# artifact and appending to its path, which is the coarser granularity.
consume = rule(
    impl = _consume_impl,
    attrs = {
        "dep": attrs.dep(),
        "projections": attrs.list(attrs.string(), default = []),
        "whole_dir_entries": attrs.list(attrs.string(), default = []),
    },
)
