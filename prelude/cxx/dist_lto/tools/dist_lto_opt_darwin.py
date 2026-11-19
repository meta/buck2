#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Python wrapper around Clang intended to optimize and codegen bitcode files
to native object files for distributed thin lto targeting darwin. This script
exists to work around Clang bugs where Clang will fail silently.
"""

import argparse
import os
import subprocess
import sys
import traceback
from typing import List

EXIT_SUCCESS, EXIT_FAILURE = 0, 1


def main(argv: List[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", help="The output native object file.", required=True)
    parser.add_argument("--input", help="The input bitcode object file.", required=True)
    parser.add_argument("--index", help="The thinlto index file.", required=True)
    parser.add_argument(
        "--compiler", help="The path to the Clang compiler binary.", required=True
    )
    parser.add_argument(
        "--shared-args",
        help="The argsfile containing unfiltered and unprocessed flags, common to all opt actions in this link.",
        required=True,
    )
    parser.add_argument(
        "--extra-outputs-args",
        help="The argsfile containing unfiltered and unprocessed flags, specifying extra outputs produced by this opt action.",
    )
    parser.add_argument(
        "--print-command",
        action="store_true",
        help="Print the clang invocation and exit.",
    )
    parser.add_argument(
        "--generate-cgdata",
        help="Save cgdata to special section in produced object file",
        action="store_true",
    )
    parser.add_argument("--read-cgdata", help="Read cgdata from the provided file")
    parser.add_argument(
        "--save-precodegen-ir",
        help="Save optimized pre-codegen IR to the specified path",
    )
    parser.add_argument(
        "--codegen-only",
        action="store_true",
        help="Skip IR optimization passes (input is already optimized pre-codegen IR)",
    )
    parser.add_argument("additional_opt_args", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv[1:])

    clang_invocation = [
        args.compiler,
        f"@{args.shared_args}",
        "-o",
        args.out,
        "-x",
        "ir",  # Without this the input file type is incorrectly inferred.
        "-c",
        args.input,
        # When lto_mode=thin/full all compile actions are passed `-flto=thin/full`. We
        # want to generate a native object file here.
        "-fno-lto",
        "-Werror=unused-command-line-argument",
        f"-fthinlto-index={args.index}",
    ]

    if args.codegen_only:
        clang_invocation.append("-fthinlto-codegen-only")

    if args.save_precodegen_ir:
        clang_invocation.append(f"-save-precodegen-ir={args.save_precodegen_ir}")

    clang_invocation += args.additional_opt_args[1:]

    if args.generate_cgdata:
        clang_invocation.extend(["-mllvm", "-codegen-data-generate"])

    if args.read_cgdata:
        clang_invocation.append(f"-fcodegen-data-use={args.read_cgdata}")

    if args.print_command:
        print(" ".join(clang_invocation))
        return EXIT_SUCCESS

    result = subprocess.run(
        clang_invocation,
        capture_output=True,
        text=True,
    )
    if result.returncode:
        print(result.stderr, file=sys.stderr)
        return result.returncode

    # Work around Clang bug where it fails silently: T187767815
    if os.stat(args.out).st_size == 0:
        print("error: clang produced empty file", file=sys.stderr)
        return EXIT_FAILURE
    if args.save_precodegen_ir and os.stat(args.save_precodegen_ir).st_size == 0:
        print("error: clang produced empty precodegen IR file", file=sys.stderr)
        return EXIT_FAILURE
    return EXIT_SUCCESS


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv))
    except subprocess.CalledProcessError as e:
        traceback.print_exc()
        sys.exit(e.returncode)
