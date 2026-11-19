# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//decls:remote_common.bzl", "remote_common")
load("@prelude//user:rule_spec.bzl", "RuleRegistrationSpec")
load("@prelude//utils:utils.bzl", "value_or")
load(":exec_deps.bzl", "HttpArchiveExecDeps")
load(":unarchive.bzl", "archive_type", "unarchive")

# Buck v2 doesn't support directories as source inputs, while v1 allows that.
# This rule fills that gap and allows to produce a directory from archive,
# which then can be used as an input for other rules.

def _impl(ctx: AnalysisContext) -> list[Provider]:
    output, sub_targets = unarchive(
        ctx,
        archive = ctx.attrs.src,
        output_name = value_or(
            value_or(ctx.attrs.out, ctx.attrs.directory_name),
            ctx.label.name,
        ),
        ext_type = archive_type(ctx.attrs.src.short_path, ctx.attrs.type),
        excludes = ctx.attrs.excludes,
        strip_prefix = ctx.attrs.strip_prefix,
        sub_targets = ctx.attrs.sub_targets,
        exec_deps = ctx.attrs.exec_deps[HttpArchiveExecDeps],
        # no need -- no http involved
        prefer_local = False,
    )

    return [
        DefaultInfo(
            default_output = output,
            sub_targets = sub_targets,
        )
    ]

registration_spec = RuleRegistrationSpec(
    name = "extract_archive",
    impl = _impl,
    attrs = remote_common.unarchive_args()
    | {
        "directory_name": attrs.option(
            attrs.string(),
            default = None,
            doc = """
            Name of the result directory, if omitted, `name` attribute will be used instead.
            Deprecated in favour of `out`.
        """,
        ),
        "src": attrs.source(
            doc = """
            .tar.gz or zip archive with the contents of the result directory
        """
        ),
    },
)
