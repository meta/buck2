#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Python wrapper around `clang` intended for use by the parallel opt phase of
a Distributed ThinLTO compilation. This script works around a LLVM bug where
LLVM will return a zero exit code in the case where ThinLTO fails with a
fatal error.

Instead of trusting the exit code of the compiler, this script checks the
output file and returns 1 if the file has zero size.
"""

import argparse
import os
import subprocess
import sys
import traceback
from typing import List, Optional

EXIT_SUCCESS, EXIT_FAILURE = 0, 1


# Filter opt related flags
def _filter_flags(clang_flags: List[str]) -> Optional[List[str]]:  # noqa: C901
    # List of llvm flags to be ignored.
    # They either don't have an valid mapping or unused during opt.
    IGNORE_OPT_FLAGS = [
        "-Wl,-plugin-opt,-function-sections",
        "-Wl,--lto-whole-program-visibility",
        "-Wl,--no-lto-whole-program-visibility",
        "-flto=thin",
    ]
    # Conservatively, we only translate llvms flags in our known list
    KNOWN_LLVM_SHARED_LIBRARY_FLAGS = ["-pie", "-shared"]

    # Start with default flags for opt.
    # The default values may change across compiler versions.
    # Make sure they are always synced with the current values.
    opt_flags = [
        # TODO(T139459294):
        # -O2 is the default optimization flag for the link-time optimizer
        # this setting matches current llvm implementation:
        # https://github.com/llvm/llvm-project/blob/main/llvm/include/llvm/LTO/Config.h#L57
        "-O2",
        "-ffunction-sections",
        "-fdata-sections",
    ]

    # Clang driver passes through lld flags with "-Wl," prefix. There are 4 type of flags with unique
    # prefixes:
    # 1. "--lto-...": these are native lld flags.
    # 2. "-plugin-opt,..." or "-plugin-opt=...": these are the aliases of the native lld flags (1).
    # 3. "-mllvm,...": these are llvm flags.
    # 4. "-plugin-opt,-..." or "-plugin-opt=-...": these are the aliases of llvm flags (3). Note that they differ from (2) and always start with "-".
    #
    # For (1) and (2), we need to convert them case by case.
    # For (3) and (4), we should be able to pass them through into the optimizer directly by prefixing "-mllvm".
    # TODO(T139448744): Cover all the flags. Check available flags using "ld.lld --help | grep -A 1 '\-\-plugin-opt='"
    PLUGIN_OPT_PREFIXES = ["-Wl,-plugin-opt,", "-Wl,-plugin-opt="]

    def _find_plugin_opt_prefix(flag: str) -> str:
        matched_prefix = [
            prefix for prefix in PLUGIN_OPT_PREFIXES if flag.startswith(prefix)
        ]
        if matched_prefix:
            return matched_prefix[0]
        return ""

    plugin_opt_to_llvm_flag_map = {
        "sample-profile=": "-fprofile-sample-use=",
        "O": "-O",
    }

    def _plugin_opt_to_clang_flag(flag: str) -> str:
        for k, v in plugin_opt_to_llvm_flag_map.items():
            if flag.startswith(k):
                return flag.replace(k, v)
        return None

    index = 0
    while index < len(clang_flags):
        raw_flag = clang_flags[index]
        flag = raw_flag.replace('"', "")
        if flag in IGNORE_OPT_FLAGS:
            index += 1
            continue
        if _find_plugin_opt_prefix(flag):
            # Convert "-Wl,-plugin-opt,...".
            flag = flag.replace(_find_plugin_opt_prefix(flag), "", 1)
            if flag.startswith("-"):
                # If flag starts with "-", it is an llvm flag. Pass it through directly.
                opt_flags.extend(["-mllvm", flag])
            else:
                flag = _plugin_opt_to_clang_flag(flag)
                if flag is None:
                    # Bail on any unknown flag.
                    print(f"error: unrecognized flag {raw_flag}")
                    return None
                opt_flags.append(flag)
        elif flag.startswith("-Wl,-mllvm,"):
            # Convert "-Wl,-mllvm,...". It is an llvm flag. Pass it through directly.
            flag = flag.replace("-Wl,-mllvm,", "", 1)
            opt_flags.extend(["-mllvm", flag])
        elif flag in KNOWN_LLVM_SHARED_LIBRARY_FLAGS:
            # The target is a shared library, `-fPIC` is needed in opt phase to correctly generate PIC ELF.
            opt_flags.append("-fPIC")
        elif flag.startswith("-f"):
            # Always pass in -f flags which are presumed to be Clang flags.
            opt_flags.append(flag)
        elif flag == "-Xlinker":
            # Handle -Xlinker -xxxx flags. -Xlinker flags are passed in two
            # lines, the first line being just "-Xlinker" and the second line
            # being the actual arg.
            if index + 1 >= len(clang_flags):
                print(
                    f"error: cannot handle -Xlinker flags {clang_flags}, "
                    "-Xlinker should be followed by an option"
                )
                return None
            if clang_flags[index + 1] == "-mllvm":
                # Validate -Xlinker
                #          -mllvm
                #          -Xlinker
                #          -xxxx    structure
                # This assumes -mllvm and its arg are provided consecutively,
                # mostly to handle the case where they come from Buck's
                # linker_flags.
                # TODO(T159109840): Generalize this logic to handle -Xlinker
                #       -mllvm -unrelated-flag -Xlinker -actual-mllvm-arg
                if (
                    index + 2 >= len(clang_flags)
                    or index + 3 >= len(clang_flags)
                    or clang_flags[index + 2] != "-Xlinker"
                ):
                    print(
                        f"error: cannot handle -Xlinker flags {clang_flags}, "
                        "-mllvm should be followed by an llvm option"
                    )
                    return None
                opt_flags.extend(["-mllvm", clang_flags[index + 3]])
                index += 3
            else:
                # Otherwise skip this -Xlinker flag and its arg
                index += 1
        index += 1
    return opt_flags


# Clean up clang flags by obtaining the cc1 flags and filtering out those unwanted.
# clang_opt_flags is mutated after calling this function.
def _cleanup_flags(clang_opt_flags: List[str]) -> List[str]:
    for i, arg in enumerate(clang_opt_flags):
        if arg.startswith("--cc="):
            # Find the clang binary path.
            clang_opt_flags[i] = arg.replace("--cc=", "")
        if arg.startswith("--log-fbcc"):
            # Remove the log-fbcc flag since the compiler is called directly below.
            clang_opt_flags.pop(i)

    # Get the cc1 flag dump with '-###'
    try:
        output = (
            subprocess.check_output(
                clang_opt_flags + ["-###"], stderr=subprocess.STDOUT
            )
            .decode()
            .splitlines()
        )
    except subprocess.CalledProcessError as e:
        print(e.output.decode())
        return None

    # Flags that may conflict with the existing bitcode attributes.
    # The value indicates if the flag is followed with a value.
    flags_to_delete = {
        "-mframe-pointer=none": False,
        "-fmath-errno": False,
        "-fno-rounding-math": False,
        "-mconstructor-aliases": False,
        "-munwind-tables": False,
        "-target-cpu": True,
        "-tune-cpu": True,
    }

    clean_output = []
    skip_next = False
    for f in output[-1].split()[1:]:
        if skip_next:
            skip_next = False
        else:
            f = f.strip('"')
            if f in flags_to_delete:
                skip_next = flags_to_delete[f]
            else:
                clean_output.append(f)
    return clean_output


# Flags that the compiler wrapper consumes and does not pass through.
_COMPILER_WRAPPER_CONSUMED_PREFIXES = (
    "--log-fbcc",
    "--fbcc-create-external-debug-info=",
    "--show-flags",
)


def _compiler_wrapper_passthrough_arg_index(opt_args: List[str]) -> int:
    """Return the index of the compiler wrapper's required ``--cc=`` argument."""
    for index, arg in enumerate(opt_args):
        if arg.startswith("--cc="):
            return index
    raise ValueError("compiler wrapper command is missing a --cc argument")


def _compiler_wrapper_prefix_end(opt_args: List[str]) -> int:
    """Return the slice end for the compiler wrapper prefix within opt_args.

    opt_args layout:
        ['--', <compiler wrapper command...>, '--cc=<compiler>', <optional wrapper-consumed args>, ...]

    Only arguments consumed by the wrapper belong in the prefix. Pass-through
    arguments like ``--target=`` are already folded into cc1 flags by
    ``_cleanup_flags`` and must not precede ``-cc1``.
    """
    end = _compiler_wrapper_passthrough_arg_index(opt_args) + 1
    while end < len(opt_args) and any(
        opt_args[end].startswith(p) for p in _COMPILER_WRAPPER_CONSUMED_PREFIXES
    ):
        end += 1
    return end


def main(argv: List[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", help="The output native object file.")
    parser.add_argument("--input", help="The input bitcode object file.")
    parser.add_argument("--index", help="The thinlto index file.")
    parser.add_argument("--split-dwarf", help="Split dwarf option.", default="none")
    parser.add_argument("--dwo", help="Split-dwarf .dwo file to be created.")
    parser.add_argument(
        "--args", help="The argsfile containing unfiltered and unprocessed flags."
    )
    parser.add_argument("--debug", action="store_true", help="Dump clang -cc1 flags.")
    parser.add_argument("opt_args", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv[1:])
    split_dwarf: str = args.split_dwarf
    dwo: str | None = args.dwo

    with open(args.args, "r") as argsfile:
        clang_opt_flags = _filter_flags(argsfile.read().splitlines())
    if clang_opt_flags is None:
        return EXIT_FAILURE

    clang_opt_flags.extend(
        [
            "-o",
            args.out,
            "-x",
            "ir",
            "-c",
            args.input,
            f"-fthinlto-index={args.index}",
        ]
    )
    if split_dwarf == "none":
        assert dwo is None
        clang_opt_flags.append("-gno-split-dwarf")
    else:
        # Note that -gsplit-dwarf=single does not create a .dwo file.
        assert (dwo is not None) == (split_dwarf == "split")
        clang_opt_flags.append(f"-gsplit-dwarf={args.split_dwarf}")

    compiler_wrapper_passthrough_arg_index = _compiler_wrapper_passthrough_arg_index(
        args.opt_args
    )
    clang_cc1_flags = _cleanup_flags(
        args.opt_args[compiler_wrapper_passthrough_arg_index:] + clang_opt_flags
    )
    if clang_cc1_flags is None:
        return EXIT_FAILURE

    if clang_cc1_flags and clang_cc1_flags[0] == "-cc1":
        # `-cc1` must be argv[1] for the clang driver.
        cc_arg = args.opt_args[compiler_wrapper_passthrough_arg_index]
        compiler_wrapper_cmd = [cc_arg[len("--cc=") :]] + clang_cc1_flags
    else:
        compiler_wrapper_prefix_end = _compiler_wrapper_prefix_end(args.opt_args)
        compiler_wrapper_cmd = (
            args.opt_args[1:compiler_wrapper_prefix_end] + clang_cc1_flags
        )

    if args.debug:
        # Print the compiler wrapper command line and exit.
        print(" ".join(compiler_wrapper_cmd))
        return EXIT_SUCCESS

    subprocess.check_call(compiler_wrapper_cmd)
    if os.stat(args.out).st_size == 0:
        print("error: opt produced empty file")
        return EXIT_FAILURE

    if dwo is not None:
        # Unfortunately some users manually add `-gno-split-dwarf` to the
        # compiler flags contradicting the --split-dwarf setting. Create an
        # empty file so buck rules are not missing the file...
        try:
            with open(dwo, "x") as _:
                pass
        except IOError:
            pass

    return EXIT_SUCCESS


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv))
    except subprocess.CalledProcessError as e:
        traceback.print_exc()
        sys.exit(e.returncode)
