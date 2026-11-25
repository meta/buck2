# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//decls:common.bzl", "buck")
load(":rule_spec.bzl", "RuleRegistrationSpec")

_NL = {
    "unix": "\n",
    "windows": "\r\n",
}

def _impl(ctx: AnalysisContext):
    # Figure out the type of newlines we're using.
    newline_type = ctx.attrs.newline
    if newline_type == "auto":
        newline_type = ctx.attrs._auto_newline

    # Join lines with newline.
    newline = _NL[newline_type]
    content = "".join([line + newline for line in ctx.attrs.content])

    output = ctx.actions.write(
        ctx.attrs.out,
        content,
        is_executable = ctx.attrs.is_executable,
        has_content_based_path = ctx.attrs.has_content_based_path,
    )

    providers = [DefaultInfo(default_output = output)]

    if ctx.attrs.is_executable:
        providers.append(RunInfo(cmd_args(output)))

    return providers

registration_spec = RuleRegistrationSpec(
    name = "write_file",
    doc = """
        Writes a text file from lines given in the build file.

        ```python
        write_file(
            name = "version",
            out = "version.txt",
            content = [
                "1.2.3",
                "stable",
            ],
        )
        ```
    """,
    impl = _impl,
    attrs = buck.labels_arg()
    | buck.contacts_arg()
    | buck.has_content_based_path_attr()
    | {
        # API based on https://github.com/bazelbuild/bazel-skylib/blob/main/docs/write_file_doc.md.
        "content": attrs.list(
            attrs.string(),
            default = [],
            doc = """
                The lines of the file. Each one is written with a line ending after it.
            """,
        ),
        "is_executable": attrs.bool(
            default = False,
            doc = """
                Whether the file is executable. If it is, `buck2 run` runs it.
            """,
        ),
        "newline": attrs.enum(
            ["auto", "unix", "windows"],
            default = "auto",
            doc = """
                The line ending: `unix` is `\\n` and `windows` is `\\r\\n`. `auto` is the one
                of the platform that the target is built for.
            """,
        ),
        "out": attrs.string(
            doc = """
                The name of the file.
            """,
        ),
        "_auto_newline": attrs.default_only(
            attrs.string(
                default = select({
                    "DEFAULT": "unix",
                    "config//os:windows": "windows",
                }),
            ),
        ),
    },
)
