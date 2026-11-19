# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _download_impl(ctx: AnalysisContext):
    output = ctx.actions.download_file(
        "download",
        ctx.attrs.url,
        sha256 = ctx.attrs.sha256,
        size_bytes = ctx.attrs.size_bytes,
        has_content_based_path = False,
    )
    return [DefaultInfo(default_output = output)]

download = rule(
    impl = _download_impl,
    attrs = {
        "sha256": attrs.string(),
        "size_bytes": attrs.option(attrs.int(), default = None),
        "url": attrs.string(),
    },
)
