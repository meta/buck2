# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _extract_impl(ctx):
    out = ctx.actions.extract_archive(
        ctx.label.name,
        ctx.attrs.src,
        type = ctx.attrs.type,
        strip_prefix = ctx.attrs.strip_prefix,
        excludes = ctx.attrs.excludes,
        has_content_based_path = ctx.attrs.content_based,
    )
    return [DefaultInfo(default_output = out)]

extract = rule(
    impl = _extract_impl,
    attrs = {
        "content_based": attrs.bool(default = False),
        "excludes": attrs.list(attrs.string(), default = []),
        "src": attrs.source(),
        "strip_prefix": attrs.option(attrs.string(), default = None),
        "type": attrs.option(attrs.string(), default = None),
    },
)

def _cat_impl(ctx):
    out = ctx.actions.declare_output("out.txt", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(
            "sh",
            "-c",
            'cat "$1/$2" > "$3"',
            "--",
            ctx.attrs.dir[DefaultInfo].default_outputs[0],
            ctx.attrs.path,
            out.as_output(),
        ),
        category = "cat",
    )
    return [
        DefaultInfo(
            default_output = out,
            sub_targets = {
                "archive": [DefaultInfo(default_outputs = ctx.attrs.dir[DefaultInfo].default_outputs)],
            },
        )
    ]

cat = rule(
    impl = _cat_impl,
    attrs = {
        "dir": attrs.dep(),
        "path": attrs.string(),
    },
)
