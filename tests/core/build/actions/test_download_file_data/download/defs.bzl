# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def declared_size():
    """The `size_bytes` a test declares through the config, if any."""
    size = read_config("test", "size_bytes")
    return int(size) if size != None else None

def _download_impl(ctx: AnalysisContext):
    output = ctx.actions.download_file(
        "download",
        ctx.attrs.url,
        sha1 = ctx.attrs.sha1,
        sha256 = ctx.attrs.sha256,
        size_bytes = ctx.attrs.size_bytes,
        has_content_based_path = ctx.attrs.has_content_based_path,
    )
    return [DefaultInfo(default_output = output)]

download = rule(
    impl = _download_impl,
    attrs = {
        "has_content_based_path": attrs.bool(default = False),
        "sha1": attrs.option(attrs.string(), default = None),
        "sha256": attrs.option(attrs.string(), default = None),
        "size_bytes": attrs.option(attrs.int(), default = None),
        "url": attrs.string(),
    },
)

def _copy_impl(ctx: AnalysisContext):
    # Reads the download through a command, which resolves the input's path from its value the
    # way every consumer does; where the command runs is the test's choice.
    out = ctx.actions.declare_output("out", has_content_based_path = False)
    ctx.actions.run(
        cmd_args(["cp", ctx.attrs.dep[DefaultInfo].default_outputs[0], out.as_output()]),
        category = "copy",
    )
    return [DefaultInfo(default_output = out)]

copy = rule(
    impl = _copy_impl,
    attrs = {
        "dep": attrs.dep(),
    },
)
