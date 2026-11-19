# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:artifacts.bzl", "ArtifactGroupInfo")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//transitions:constraint_overrides.bzl", "constraint_overrides")

def filegroup_impl(ctx):
    """
    Creates a directory that contains links to the list of srcs

    The output is a directory that uses `out` for its name, if provided, or the rule name if not.
    Each symlink is based on the `short_path` for the provided `src`.
    """
    output_name = ctx.attrs.out if ctx.attrs.out else ctx.label.name
    has_content_based_path = ctx.attrs.has_content_based_path

    if type(ctx.attrs.srcs) == type({}):
        srcs = ctx.attrs.srcs
    else:
        srcs = {}
        for src in ctx.attrs.srcs:
            existing = srcs.get(src.short_path)
            if existing != None and existing != src:
                fail("filegroup {} has srcs with duplicate names: {} and {}".format(ctx.label, src, srcs[src.short_path]))
            srcs[src.short_path] = src

    # It seems that buck1 always copies, and that's important for Python rules
    if ctx.attrs.copy:
        output = ctx.actions.copied_dir(
            output_name,
            srcs,
            executable_bit_override = ctx.attrs.executable_bit_override,
            has_content_based_path = has_content_based_path,
        )
    elif ctx.attrs.executable_bit_override != None:
        fail("filegroup does not allow specifying `executable_bit_override` with `copy = False`")
    else:
        output = ctx.actions.symlinked_dir(
            output_name,
            srcs,
            has_content_based_path = has_content_based_path,
        )

    if type(ctx.attrs.srcs) == type([]):
        artifacts = ctx.attrs.srcs
    else:
        artifacts = [output.project(name, hide_prefix = True) for name in srcs]

    return [
        DefaultInfo(default_output = output),
        ArtifactGroupInfo(artifacts = artifacts),
    ]

filegroup = prelude_rule(
    name = "filegroup",
    impl = filegroup_impl,
    docs = """
        This rule provides access to a set of files.

         Files are accessible to `genrule()`s by using their relative path
         after a `$(location)` string parameter macro.

         Other rules may handle `filegroup()` rules natively for attributes
         such as resources.
    """,
    examples = """
        In this example a target exports `.xml` files from all subdirectories
         in `resources`.

        ```
        filegroup(
          name = 'example',
          srcs = glob(['resources/**/*.xml']),
        )

        genrule(
          name = 'process_xml',
          out = 'processed.xml',
          cmd = '$(exe //example:tool) -in $(location :example)/resources/file.xml > $OUT',
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "srcs": attrs.named_set(
                attrs.source(allow_directory = True),
                sorted = False,
                default = [],
                doc = """
                The set of files to include in this rule.
            """,
            ),
            "copy": attrs.bool(default = True),
            "executable_bit_override": attrs.option(
                attrs.bool(),
                default = None,
                doc = """
                Override the executable bit for every file in the filegroup. If not set, the executable bits are preserved.
                Cannot be used if `copy` is set to false.
            """,
            ),
            "out": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                The name of the output directory. Defaults to the rule's name.
            """,
            ),
        }
        | buck.has_content_based_path_attr()
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
        | constraint_overrides.attributes
    ),
    cfg = constraint_overrides.transition,
)
