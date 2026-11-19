# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

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
