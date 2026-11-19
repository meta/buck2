# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Output layouts for the `gen` rule, keyed by its `variant` attribute. A layout
# is a list of (path, contents) pairs, one per declared output artifact: a dict
# declares a directory artifact holding those files, a string declares a file
# artifact with that content.
#
# The two layouts of a pair (`X_1` and `X_2`) belong to the target a test flips
# between builds, and place artifact boundaries at different paths within the
# same on-disk output directory. Their contents differ so that the second build
# cannot pass by leaving the first build's bytes in place.
LAYOUTS = {
    # One directory artifact splits into a file artifact and a smaller
    # directory artifact, both underneath the path it used to occupy.
    "dir_to_files_1": [("out", {"a": "A1", "sub/keep": "K1", "sub/stale": "S1"})],
    "dir_to_files_2": [("out/a", "A2"), ("out/sub", {"keep": "K2"})],
    # ... and the reverse.
    "files_to_dir_1": [("out/a", "A1"), ("out/b", "B1")],
    "files_to_dir_2": [("out", {"a": "A2", "c": "C2"})],
    # As above, but consumed by a local action through `list_dir`.
    "listed_dir_1": [("out", {"keep": "K1", "stale": "S1"})],
    "listed_dir_2": [("out", {"keep": "K2"})],
    # Boundaries stay put; the directory artifact just loses an entry.
    "shrinking_dir_1": [("out", {"keep.txt": "KEEP1", "stale.txt": "STALE"})],
    "shrinking_dir_2": [("out", {"keep.txt": "KEEP2"})],
}

_WRITE_FILE = """
import sys

with open(sys.argv[1], "w") as f:
    f.write(sys.argv[2])
"""

_WRITE_DIR = """
import os
import sys

root = sys.argv[1]
os.makedirs(root, exist_ok=True)
for entry in sys.argv[2:]:
    name, _, content = entry.partition("=")
    path = os.path.join(root, name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(content)
"""

def _declare(ctx, path, dir):
    components = path.split("/")
    if len(components) == 1:
        return ctx.actions.declare_output(path, dir = dir, has_content_based_path = False)
    return ctx.actions.declare_output(
        "/".join(components[:-1]),
        components[-1],
        dir = dir,
        has_content_based_path = False,
    )

def _gen_impl(ctx):
    outs = []
    for path, contents in LAYOUTS[ctx.attrs.variant]:
        if type(contents) == type({}):
            out = _declare(ctx, path, dir = True)
            ctx.actions.run(
                cmd_args(
                    ["fbpython", "-c", _WRITE_DIR, out.as_output()],
                    ["{}={}".format(name, content) for name, content in contents.items()],
                ),
                category = "gen",
                identifier = path,
            )
        else:
            out = _declare(ctx, path, dir = False)
            ctx.actions.run(
                cmd_args(["fbpython", "-c", _WRITE_FILE, out.as_output(), contents]),
                category = "gen",
                identifier = path,
            )
        outs.append(out)
    return [DefaultInfo(default_outputs = outs)]

gen = rule(
    impl = _gen_impl,
    attrs = {
        "variant": attrs.string(),
    },
)

_LIST_DIR = """
import os
import sys

with open(sys.argv[1], "w") as out:
    out.write(",".join(sorted(os.listdir(sys.argv[2]))))
"""

def _list_dir_impl(ctx):
    dep = ctx.attrs.dep[DefaultInfo].default_outputs[0]
    out = ctx.actions.declare_output("listing.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["fbpython", "-c", _LIST_DIR, out.as_output(), dep]),
        category = "list_dir",
        # Force the directory to be materialized on disk rather than read from
        # wherever the action would otherwise have run.
        local_only = True,
    )
    return [DefaultInfo(default_output = out)]

# Writes the sorted entry names of `dep`'s directory artifact, so that a test can
# assert on what a local action saw in it.
list_dir = rule(
    impl = _list_dir_impl,
    attrs = {
        "dep": attrs.dep(),
    },
)
