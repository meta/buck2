# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _cp_impl(ctx: AnalysisContext):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            "fbpython",
            "-c",
            "import shutil, sys; from pathlib import Path; shutil.copyfile(Path(sys.argv[1]), Path(sys.argv[2]))",
            ctx.attrs.src,
            out.as_output(),
        ),
        category = "cp",
        local_only = ctx.attrs.local_only,
    )

    return [
        DefaultInfo(default_output = out),
    ]

cp = rule(
    impl = _cp_impl,
    attrs = {
        "local_only": attrs.bool(default = False),
        "src": attrs.source(),
    },
)

def _cp_via_builtin_impl(ctx: AnalysisContext):
    out = ctx.actions.copy_file("out", ctx.attrs.src, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

cp_via_builtin = rule(
    impl = _cp_via_builtin_impl,
    attrs = {
        "src": attrs.source(),
    },
)

def _stat_path_impl(ctx: AnalysisContext):
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    if ctx.attrs.project != None:
        project = " / '" + ctx.attrs.project + "'"
    else:
        project = ""
    ctx.actions.run(
        cmd_args(
            "fbpython",
            "-c",
            cmd_args(
                "import sys",
                "from pathlib import Path",
                "p = Path(sys.argv[2])" + project,
                "p.stat()",  # Just make sure this succeeds
                "open(sys.argv[1], 'w').write(str(p.is_symlink()))",
                delimiter = "; ",
            ),
            out.as_output(),
            ctx.attrs.path,
        ),
        category = "stat_path",
    )
    return [
        DefaultInfo(default_output = out),
    ]

stat_path = rule(
    impl = _stat_path_impl,
    attrs = {
        "path": attrs.source(allow_directory = True),
        "project": attrs.option(attrs.string(), default = None),
    },
)
