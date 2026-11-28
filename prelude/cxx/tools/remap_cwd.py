#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

"""
Usage: remap_cwd.py [--gnu-assembler] path/to/compiler [args...]

Runs `path/to/compiler [args...] -ffile-prefix-map=$PWD=.`, plus `-Wa,--debug-prefix-map=$PWD=.`
when `--gnu-assembler` is given.

Flags are appended so that compiler wrappers using "$@" see the real binary as $1. Using $PWD=.
remaps both DW_AT_comp_dir (which GCC sets to exactly $PWD without trailing slash) and source file
paths. -Wa,--debug-prefix-map is needed for GCC because it does not pass -ffile-prefix-map through
to the assembler for .s/.S/.sx files. Clang's integrated assembler honours -ffile-prefix-map, and
the Clang driver rejects --debug-prefix-map in -Wa, so the flag is only added on request.
"""

import os
import subprocess
import sys


if __name__ == "__main__":
    cwd = os.getcwd()

    args = sys.argv[1:]
    remap_flags = [f"-ffile-prefix-map={cwd}=."]
    if args and args[0] == "--gnu-assembler":
        args = args[1:]
        remap_flags.append(f"-Wa,--debug-prefix-map={cwd}=.")

    ret = subprocess.call([*args, *remap_flags])
    sys.exit(ret)
