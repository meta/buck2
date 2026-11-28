#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import codecs
import os
import subprocess
import sys
import tempfile


def unquote(argument):
    if len(argument) > 2 and argument[0] == '"' and argument[-1] == '"':
        return argument[1:-1]
    else:
        return argument


def is_relative_buck_out_path(argument):
    return argument.startswith("buck-out\\")


def is_library_path(argument):
    return argument.endswith(".lib")


def is_whole_archive_option(argument):
    return argument.startswith("/WHOLEARCHIVE")


def expand_library_path(argument, full_library_paths):
    # `/WHOLEARCHIVE:<library>` names the library either by the basename of a
    # library given earlier on the command line, or by its path (which is what
    # `get_link_whole_args` in prelude/cxx/linker.bzl passes). Expand the former
    # and keep the latter.
    option, separator, library = argument.partition(":")
    if not separator:
        return argument
    return option + ":" + full_library_paths.get(library, library)


# NOTE: it does not check for cycles
def expand_args(arguments):
    expanded = []
    for argument in arguments:
        unquoted_argument = unquote(argument)
        if unquoted_argument.startswith("@"):
            filename = unquoted_argument[1:]
            with open(filename, mode="rb") as f:
                file_content = f.read()
            if file_content.startswith(codecs.BOM_UTF16):
                string = file_content.decode("utf-16")
            else:
                string = file_content.decode("utf-8")
            expanded.extend(expand_args(string.splitlines()))
        else:
            expanded.append(argument)
    return expanded


def rewrite_args(arguments, working_dir):
    full_library_paths = {}
    new_args = []
    for argument in arguments:
        argument = unquote(argument)

        if is_whole_archive_option(argument):
            # LLVM linker expects full path in /WHOLEARCHIVE option while MSVC
            # expects just the filename. That's why we need to expand the path.
            argument = expand_library_path(argument, full_library_paths)
            option, separator, library = argument.partition(":")
            if separator:
                full_library_paths.setdefault(os.path.basename(library), library)
                if is_relative_buck_out_path(library):
                    argument = option + ":" + os.path.join(working_dir, library)
        elif is_library_path(argument):
            # Save library paths to support expanding them afterwards
            library_name = os.path.basename(argument)
            full_library_paths[library_name] = argument

        # Make relative paths absolute for support paths longer than 260 chars.
        if is_relative_buck_out_path(argument):
            argument = os.path.join(working_dir, argument)

        new_args.append(argument)
    return new_args


def main():
    linker_real, rest = sys.argv[1], sys.argv[2:]
    working_dir = os.getcwd()

    new_args = rewrite_args(expand_args(rest), working_dir)

    # Based on rustc's @linker-arguments file construction.
    # https://github.com/rust-lang/rust/blob/1.69.0/compiler/rustc_codegen_ssa/src/back/link.rs#L1383-L1407
    quoted_new_args = ('"{}"\n'.format(arg) for arg in new_args)
    new_args_content = "".join(quoted_new_args).encode("utf-16")

    debug = "[\n" + "".join('    "{}"\n'.format(arg) for arg in new_args) + "]"
    print(f"linker_args = {debug}", file=sys.stderr)

    with tempfile.NamedTemporaryFile(mode="wb", delete=False) as new_args_file:
        new_args_file.write(new_args_content)
        new_args_file_name = new_args_file.name

    command = [linker_real, "@" + new_args_file_name]
    subprocess.run(command, check=True)


if __name__ == "__main__":
    main()
