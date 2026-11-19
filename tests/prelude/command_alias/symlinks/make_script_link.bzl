# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl(ctx):
    base = ctx.attrs.base[DefaultInfo].default_outputs[0]
    final_component = base.short_path.split("/")[-1]
    if ctx.attrs.via_parent:
        to_parent = ctx.actions.declare_output("to_parent", has_content_based_path = False)
        ctx.actions.run(
            cmd_args(
                "fbpython",
                "-c",
                cmd_args(
                    "from pathlib import Path;",
                    "import sys;",
                    "import os;",
                    "rel = os.path.relpath(Path(sys.argv[2]), Path(sys.argv[1]).parent);",
                    "Path(sys.argv[1]).symlink_to(rel, target_is_directory=True)",
                    delimiter = " ",
                ),
                to_parent.as_output(),
                cmd_args(base, parent = 1),
            ),
            category = "make_symlink_to_parent",
            # Must run locally because RE does not correctly preserve output symlinks on Windows
            local_only = True,
            allow_cache_upload = False,
            # Dodge a bad cache artifact that has the problem above
            env = {"CACHE_BUSTER": "1"},
        )
        out = cmd_args(to_parent, final_component, delimiter = ctx.attrs.path_sep)
    else:
        out = ctx.actions.symlink_file(final_component, base, has_content_based_path = False)
    args = cmd_args(out, hidden = ctx.attrs.base[DefaultInfo].other_outputs)
    return [DefaultInfo(), RunInfo(args = args)]

make_script_link = rule(
    impl = _impl,
    attrs = {
        "base": attrs.dep(),
        "path_sep": attrs.string(
            default = select({
                "DEFAULT": "/",
                "ovr_config//os:windows": "\\",
            })
        ),
        "via_parent": attrs.bool(),
    },
)
