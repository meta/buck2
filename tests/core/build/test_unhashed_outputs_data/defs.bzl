# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _projected_output_impl(ctx):
    f = ctx.actions.write("f", "", has_content_based_path = False)
    d = ctx.actions.copied_dir("dir", {"file": f}, has_content_based_path = False)
    return [
        DefaultInfo(default_output = d.project("file")),
    ]

projected_output = rule(
    impl = _projected_output_impl,
    attrs = {},
)

_PROJECTED_SYMLINK_DIR = """
import os
import sys

root = sys.argv[1]
os.makedirs(root, exist_ok=True)
with open(os.path.join(root, "target"), "w") as f:
    f.write("contents")
os.symlink("target", os.path.join(root, "profile"))
"""

def _projected_symlink_output_impl(ctx):
    out = ctx.actions.declare_output(
        "out",
        dir = True,
        has_content_based_path = True,
    )
    ctx.actions.run(
        cmd_args(
            "fbpython",
            "-c",
            _PROJECTED_SYMLINK_DIR,
            out.as_output(),
        ),
        category = "projected_symlink_output",
    )
    return [DefaultInfo(default_output = out.project("profile"))]

projected_symlink_output = rule(
    impl = _projected_symlink_output_impl,
    attrs = {},
)

def _simple_build_impl(ctx):
    f = ctx.actions.write(ctx.attrs.filename, "", has_content_based_path = ctx.attrs.has_content_based_path)
    others = []
    for d in ctx.attrs.deps:
        others.extend(d[DefaultInfo].default_outputs)
    return [
        DefaultInfo(default_output = f, other_outputs = others),
    ]

simple_build = rule(
    impl = _simple_build_impl,
    attrs = {
        "deps": attrs.list(attrs.dep(), default = []),
        "filename": attrs.string(default = "out.txt"),
        "has_content_based_path": attrs.bool(default = False),
    },
)
