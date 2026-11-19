# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

FooInfo = provider(fields = ["args", "out"])

def _simple_write_impl(ctx):
    out = ctx.actions.write("out.txt", "contents", has_content_based_path = False)
    args = cmd_args([out])
    return [
        FooInfo(args = args, out = out),
        DefaultInfo(default_output = out),
    ]

def _write_file_impl(ctx):
    if ctx.attrs.name == "uses_declared_output":
        declared = ctx.actions.declare_output(ctx.attrs.out, has_content_based_path = False)
        output = ctx.actions.write(declared, ctx.attrs.content)
    elif ctx.attrs.name == "uses_declared_output_as_output":
        declared = ctx.actions.declare_output(ctx.attrs.out, has_content_based_path = False)
        output = ctx.actions.write(declared.as_output(), ctx.attrs.content)
    elif ctx.attrs.name == "declares_output":
        output = ctx.actions.write(ctx.attrs.out, ctx.attrs.content, has_content_based_path = False)
    elif ctx.attrs.name == "is_executable":
        output = ctx.actions.write(ctx.attrs.out, ctx.attrs.content, is_executable = True, has_content_based_path = False)
    elif ctx.attrs.name == "writes_array_of_commands":
        cmd = [ctx.attrs.dep[FooInfo].out, ctx.attrs.content]
        output = ctx.actions.write(ctx.attrs.out, cmd, has_content_based_path = False)
    elif ctx.attrs.name == "writes_command_lines":
        cmd = [ctx.attrs.dep[FooInfo].out, ctx.attrs.content]
        output = ctx.actions.write(ctx.attrs.out, cmd_args(cmd), has_content_based_path = False)
    elif ctx.attrs.name == "writes_frozen_command_lines":
        output = ctx.actions.write(ctx.attrs.out, ctx.attrs.dep[FooInfo].args, has_content_based_path = False)
    elif ctx.attrs.name == "with_inputs_and_copy":
        output1 = ctx.actions.write("intermediate.txt", ctx.attrs.content, has_content_based_path = False)
        output2 = ctx.actions.declare_output(ctx.attrs.out, has_content_based_path = False)

        # Create script with output1 as its associated artifact
        cmd = cmd_args(output1, format = "import sys; fp1=open('{}','r'); all=fp1.read(); fp2=open(sys.argv[1], 'w'); fp2.write(all);")

        # Replace \ with \\ for Windows compatibility
        cmd = cmd_args(cmd, replace_regex = ("\\\\\\b", "\\\\"))
        script = ctx.actions.write(
            "script.py",
            [cmd],
            with_inputs = True,
            has_content_based_path = False,
        )

        # Read output1 and write back into output2. Output1 should be included as an associated artifact here so we do not need to add it as hidden
        cmd = cmd_args(["fbpython", script, output2.as_output()])
        ctx.actions.run(cmd, category = "test")
        return [DefaultInfo(default_output = output2)]
    elif ctx.attrs.name == "fails_on_invalid_contents":
        output = ctx.actions.write(ctx.attrs.out, {}, has_content_based_path = False)
    elif ctx.attrs.name == "fails_on_invalid_output":
        output = ctx.actions.write([], ctx.attrs.content)
    elif ctx.attrs.name == "writes_absolute":
        content = [ctx.attrs.dep[FooInfo].out]
        output = ctx.actions.write(ctx.attrs.out, content, absolute = True, has_content_based_path = False)
    else:
        fail("invalid test")
    return [DefaultInfo(default_output = output)]

write_file = rule(
    impl = _write_file_impl,
    attrs = {
        "content": attrs.string(default = "some content"),
        "dep": attrs.option(attrs.dep(providers = [FooInfo]), default = None),
        "exe": attrs.bool(default = False),
        "out": attrs.string(default = "out.txt"),
    },
)

simple_write = rule(
    impl = _simple_write_impl,
    attrs = {},
)
