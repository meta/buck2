# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _copy_impl(ctx):
    out = ctx.actions.declare_output("action_output", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            ["cp", ctx.attrs.src, out.as_output()],
            hidden = ctx.attrs.hidden,
        ),
        category = "cp",
    )

    return [DefaultInfo(default_output = out)]

copy = rule(
    impl = _copy_impl,
    attrs = {
        "hidden": attrs.source(),
        "src": attrs.source(),
    },
)

def _download(ctx: AnalysisContext):
    download = ctx.actions.declare_output("download", has_content_based_path = False)
    ctx.actions.download_file(download, ctx.attrs.url, sha1 = ctx.attrs.sha1)

    return [
        DefaultInfo(default_output = download),
    ]

# `url` and `sha1` come from `test.url` / `test.sha1` in the config; they are optional only so
# that the package loads for the tests that do not build `:download`.
download = rule(
    impl = _download,
    attrs = {
        "sha1": attrs.option(attrs.string(), default = None),
        "url": attrs.option(attrs.string(), default = None),
    },
)
