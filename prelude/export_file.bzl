# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//transitions:constraint_overrides.bzl", "constraint_overrides")
# Implementation of the `export_file` build rule.

def export_file_impl(ctx: AnalysisContext) -> list[DefaultInfo]:
    # mode is "copy" or "reference", defaulting to copy
    copy = ctx.attrs.mode != "reference"

    if copy:
        dest = ctx.label.name if ctx.attrs.out == None else ctx.attrs.out
        output = ctx.actions.copy_file(
            dest,
            ctx.attrs.src,
            executable_bit_override = ctx.attrs.executable_bit_override,
            has_content_based_path = ctx.attrs.has_content_based_path,
        )
    elif ctx.attrs.out != None:
        fail("export_file does not allow specifying `out` without also specifying `mode = 'copy'`")
    elif ctx.attrs.executable_bit_override != None:
        fail("export_file does not allow specifying `executable_bit_override` without also specifying `mode = 'copy'`")
    else:
        output = ctx.attrs.src
    return [DefaultInfo(default_output = output)]

ExportFileDescriptionMode = ["reference", "copy"]

export_file = prelude_rule(
    name = "export_file",
    impl = export_file_impl,
    docs = """
        An `export_file()` takes a single file or folder and exposes it so other rules can
         use it.
    """,
    examples = """
        The best way to see how the `export_file()` rule works is with some examples. The
         common case is:

        ```
        export_file(
          name = 'example.html',
        )

        # This is equivalent to

        export_file(
          name = 'example.html',
          src = 'example.html',
          out = 'example.html',
        )
        ```

         It is sometimes useful to refer to the file not by its path, but by a more logical name:

        ```
        export_file(
          name = 'example',
          src = 'example.html',
        )

        # This is equivalent to

        export_file(
          name = 'example',
          src = 'example.html',
          out = 'example.html',
        )
        ```

         Finally, there are occasions where you want to export a file more than once but want to copy it to
         a different name for each output:

        ```
        export_file(
          name = 'runner',
          src = 'RemoteRunner.html',
        )

        export_file(
          name = 'runner_hta',
          src = 'RemoteRunner.html',
          out = 'RemoteRunner.hta',
        )
        ```

         Using the `export_file()` rule is also simple:

        ```
        export_file(
          name = 'example',
          src = 'example.html',
        )

        genrule(
          name = 'demo',
          out = 'result.html',
          cmd = 'cp $(location :example) $OUT',
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "src": attrs.option(
                attrs.source(allow_directory = True),
                default = None,
                doc = """
                The path to the file that should be exported.
            """,
            ),
            "out": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                The name which the file will be called if another rule depends on it instead of the name it
                 already has.
            """,
            ),
            "executable_bit_override": attrs.option(
                attrs.bool(),
                default = None,
                doc = """
                Override the executable bit of the file. If not set, the executable bit is preserved.
            """,
            ),
            "mode": attrs.option(
                attrs.enum(ExportFileDescriptionMode),
                default = None,
                doc = """
                How files are referenced internally in buck.
                 If set to 'copy', then a full copy will be made into the new location in buck-out.
                 If set to 'reference', the original file will be used by internal build rules in-place.
                 However, this mode does not work across repositories or if the 'out' property is set.
                 For read-only operations, 'reference' can be more performant.
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
