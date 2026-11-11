# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _copy_file_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.copy_file("out.txt", ctx.attrs.src, has_content_based_path = False)
    return [DefaultInfo(default_output = out)]

copy_file = rule(impl = _copy_file_impl, attrs = {"src": attrs.source()})

def _writer_impl(ctx: AnalysisContext) -> list[Provider]:
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    ctx.actions.run(
        [
            "fbpython",
            "-c",
            "import os, pathlib, sys, time; out, started, release = sys.argv[1:]; pathlib.Path(started).write_text(str(os.getpid()));\nwhile not pathlib.Path(release).exists():\n pathlib.Path(out).write_text('running'); time.sleep(0.05)\npathlib.Path(out).write_text('finished')",
            out.as_output(),
            ctx.attrs.started,
            ctx.attrs.release,
        ],
        category = "writer",
        local_only = True,
    )
    return [DefaultInfo(default_output = out)]

writer = rule(impl = _writer_impl, attrs = {"release": attrs.string(), "started": attrs.string()})
