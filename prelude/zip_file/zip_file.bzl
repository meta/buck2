# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "OnDuplicateEntry", "buck")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load(":zip_file_toolchain.bzl", "ZipFileToolchainInfo")

def _zip_file_impl(ctx: AnalysisContext) -> list[Provider]:
    """
     zip_file() rule implementation

    Args:
        ctx: rule analysis context
    Returns:
        list of created providers
    """

    zip_file_toolchain = ctx.attrs._zip_file_toolchain[ZipFileToolchainInfo]
    create_zip_tool = zip_file_toolchain.create_zip

    zip_output_name = ctx.attrs.out if ctx.attrs.out else "{}.zip".format(ctx.label.name)
    output = ctx.actions.declare_output(zip_output_name, has_content_based_path = True)

    on_duplicate_entry = ctx.attrs.on_duplicate_entry
    entries_to_exclude = ctx.attrs.entries_to_exclude
    hardcode_permissions_for_deterministic_output = ctx.attrs.hardcode_permissions_for_deterministic_output
    zip_srcs = ctx.attrs.zip_srcs
    srcs = ctx.attrs.srcs

    create_zip_cmd = [
        create_zip_tool,
        "--output_path",
        output.as_output(),
        "--on_duplicate_entry",
        on_duplicate_entry if on_duplicate_entry else "overwrite",
    ]

    if srcs:
        # add artifact and is_source flag pair
        srcs_file_cmd = cmd_args(
            [[src, src.short_path, str(src.is_source)] for src in srcs],
        )
        entries_file = ctx.actions.write("entries", srcs_file_cmd, has_content_based_path = True)

        create_zip_cmd.append("--entries_file")
        create_zip_cmd.append(entries_file)
        create_zip_cmd.append(cmd_args(hidden = srcs))

    if zip_srcs:
        create_zip_cmd.append("--zip_sources")
        create_zip_cmd.append(zip_srcs)

    if entries_to_exclude:
        create_zip_cmd.append("--entries_to_exclude")
        create_zip_cmd.append(entries_to_exclude)

    if hardcode_permissions_for_deterministic_output:
        create_zip_cmd.append("--hardcode_permissions_for_deterministic_output")

    ctx.actions.run(cmd_args(create_zip_cmd), category = "zip")

    return [DefaultInfo(default_output = output)]

zip_file = prelude_rule(
    name = "zip_file",
    impl = _zip_file_impl,
    docs = """
        A `zip_file()` allows builds to create basic zip files in
         a platform-agnostic way.
    """,
    examples = """
        This example will create a simple zip file.

        ```
        zip_file(
          # The output will be "example.zip"
          name = 'example',
          srcs =
            # These files will be found in the zip under "dir/"
            glob(['dir/**/*']) +
            [
              # Imagine this generates the output
              # "buck-out/gen/foo/hello.txt". This output will
              # be found in the zip at "hello.txt"
              '//some/other:target',

            ],
          zip_srcs = [
             # The contents of this zip will be added to the generated zip.
            'amazing-library-1.0-sources.zip',
          ],
          entries_to_exclude = [
            "com/example/amazinglibrary/Source1.java",
          ],
        )
        ```
        If you were to examine the generated zip, the contents would look
        something like (assuming the output of
        "`//some/other:target`" was a file who's path ended with
         `hello.txt`, the "`dir`" glob found two files,
        and "`amazing-library-1.0-sources.zip`" contained two Java
        source files):

        ```
        dir/file1.txt
        dir/subdir/file2.txt
        hello.txt
        com/example/amazinglibrary/Source2.java
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "out": attrs.string(
                default = "",
                doc = """
                The name of the zip file that should be generated. This allows
                 builds to use a meaningful target name coupled with a meaningful zip
                 file name. The default value takes the rule's `name` and
                 appends `.zip`.
            """,
            ),
            "srcs": attrs.list(
                attrs.source(),
                default = [],
                doc = """
                The set of files to include in the zip.

                 Each `src` will be added to the zip as follows:
                 * If the `src` is the output of another rule, the output
                 will be included using just the output's file name.
                 * If the `src` is a file relative to the rule's
                 declaration, it will be included in the zip with its relative file
                 name.
            """,
            ),
            "zip_srcs": attrs.list(
                attrs.source(),
                default = [],
                doc = """
                The set of zip files whose content to include in the output zip file.

                 Note that the order of files in `zip_srcs` matters because the same zip entry can be
                 included from multiple files. See the `on_duplicate_entry` argument to learn how to
                 control the behavior when there are multiple entries with the same name.

                 The entries from `zip_srcs` are added before files from `srcs`.
            """,
            ),
            "entries_to_exclude": attrs.list(
                attrs.regex(),
                default = [],
                doc = """
                List of regex expressions that describe entries that should not be included in the output zip file.

                 The regexes must be defined using `java.util.regex.Pattern` syntax.
            """,
            ),
            "hardcode_permissions_for_deterministic_output": attrs.option(
                attrs.bool(),
                default = None,
                doc = """
                If set to true, Buck hardcodes the permissions in order to ensures that all files have the same
                permissions regardless of the platform on which the zip was generated.
            """,
            ),
            "on_duplicate_entry": attrs.enum(
                OnDuplicateEntry,
                default = "overwrite",
                doc = """
                Action performed when Buck detects that zip\\_file input contains multiple entries with the same
                 name.

                 The valid values are:
                 * `overwrite` (default): the last entry overwrites all previous entries with
                 the same name.
                * `append`: all entries are added to the output file.
                * `fail`: fail the build when duplicate entries are present.
            """,
            ),
        }
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | {
            "_zip_file_toolchain": toolchains_common.zip_file(),
        }
    ),
)
