# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl(ctx):
    if ctx.attrs.src != None and ctx.attrs.write_executable_bit != None:
        fail("Cannot specify both src and write_executable_bit")
    elif ctx.attrs.src != None:
        src = ctx.attrs.src
    elif ctx.attrs.write_executable_bit != None:
        src = ctx.actions.write("empty.txt", "", is_executable = bool(ctx.attrs.write_executable_bit), has_content_based_path = False)
    else:
        fail("Must specify either src or write_executable_bit")

    copied = ctx.actions.copy_file(ctx.label.name, src, executable_bit_override = ctx.attrs.executable_bit_override, has_content_based_path = False)
    perms = ctx.actions.declare_output("{}_perms.txt".format(ctx.label.name), has_content_based_path = False)
    script = cmd_args(
        "ls -lR",
        copied,
        "| cut -d ' ' -f 1",
        "| grep '^-' >",
        perms.as_output(),
        delimiter = " ",
    )
    ctx.actions.run(cmd_args("bash", "-c", script), category = "test")
    return [
        DefaultInfo(default_output = perms),
    ]

perms_of_copied_file = rule(
    impl = _impl,
    attrs = {
        "executable_bit_override": attrs.option(attrs.bool(), default = None),
        "src": attrs.option(attrs.source(allow_directory = True), default = None),
        "write_executable_bit": attrs.option(attrs.bool(), default = None),
    },
)

def gen_test_cases():
    for executable_bit_override in [None, True, False]:
        for write_executable_bit in [None, True, False]:
            for src, _ in [
                (None, None),
                ("files/is_executable.sh", True),
                ("files/not_executable.sh", False),
                ("files/executable_scripts", True),
                ("files/not_executable_scripts", False),
            ]:
                if src == None and write_executable_bit == None:
                    continue
                if src != None and write_executable_bit != None:
                    continue
                name = "perms_{}_{}_{}".format(executable_bit_override, write_executable_bit, src)
                perms_of_copied_file(
                    name = name,
                    executable_bit_override = executable_bit_override,
                    src = src,
                    write_executable_bit = write_executable_bit,
                )
