# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _http_archive_impl(ctx: AnalysisContext):
    download = ctx.actions.declare_output("download", has_content_based_path = False)
    ctx.actions.download_file(download, ctx.attrs.urls[0], sha1 = ctx.attrs.sha1)

    output = ctx.actions.declare_output("output", has_content_based_path = False)
    ctx.actions.run(["cp", download, output.as_output()], category = "cp")

    return [
        DefaultInfo(default_output = output),
    ]

http_archive = rule(
    impl = _http_archive_impl,
    attrs = {
        "sha1": attrs.string(),
        "urls": attrs.list(attrs.string()),
    },
)

def _cas_artifact_impl(ctx: AnalysisContext):
    out = ctx.actions.cas_artifact(
        ctx.label.name,
        ctx.attrs.digest,
        ctx.attrs.use_case,
        expires_after_timestamp = ctx.attrs.expires_after_timestamp,
        is_tree = ctx.attrs.is_tree,
        is_directory = ctx.attrs.is_directory,
    )
    return [DefaultInfo(default_output = out)]

cas_artifact = rule(
    impl = _cas_artifact_impl,
    attrs = {
        "digest": attrs.string(),
        "expires_after_timestamp": attrs.int(),
        "is_directory": attrs.bool(default = False),
        "is_tree": attrs.bool(default = False),
        "use_case": attrs.string(),
    },
)

def _genrule_impl(ctx: AnalysisContext):
    out = ctx.actions.declare_output(ctx.attrs.out, has_content_based_path = False)

    # Use environment variable to pass output path
    ctx.actions.run(
        cmd_args("sh", "-c", ctx.attrs.cmd, hidden = ctx.attrs.srcs),
        category = "genrule",
        env = {"OUT": out.as_output()},
        allow_offline_output_cache = ctx.attrs.allow_offline_output_cache,
    )
    return [DefaultInfo(default_output = out)]

genrule = rule(
    impl = _genrule_impl,
    attrs = {
        "allow_offline_output_cache": attrs.bool(default = False),
        "cmd": attrs.string(),
        "out": attrs.string(),
        "srcs": attrs.list(attrs.source(), default = []),
    },
)
