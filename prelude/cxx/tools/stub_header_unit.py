#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Usage: stub_header_unit.py [...] -o <output> [-MF <depfile>] [...]

Runs the preprocessor and hashes the output to produce a stub file.

Transforms compiler arguments:
- Intercepts -o
- Replaces --precompile with -E and redirects output to a temporary file
  (some compiler wrappers suppress stdout)
- Intercepts -MF to use a temporary file
"""

from __future__ import annotations

import hashlib
import os
import shlex
import subprocess
import sys
import tempfile

DEBUG = os.environ.get("STUB_HEADER_UNIT_DEBUG") == "1"


def parse_and_transform_args(
    args: list[str],
    temp_depfile: str,
) -> tuple[str | None, str | None, list[str]]:
    """
    Parse arguments to extract --cc, -o, and transform remaining args.

    Args:
        args: Command line arguments
        temp_depfile: Path to temporary depfile to use for -MF

    Returns:
        (output_file, transformed_args)
    """
    output_file: str | None = None
    transformed_args: list[str] = []

    i = 0
    while i < len(args):
        arg = args[i]

        if arg == "-o":
            if i + 1 < len(args):
                output_file = args[i + 1]
                i += 2
            else:
                i += 1
        elif arg.startswith("-o"):
            # Handle -o<file> without space
            output_file = arg[2:]
            i += 1
        elif arg == "--precompile":
            # Replace --precompile with -E
            transformed_args.append("-E")
            i += 1
        elif arg == "-MF":
            # Replace -MF argument with our temporary file
            if i + 1 < len(args):
                transformed_args.append("-MF")
                transformed_args.append(temp_depfile)
                i += 2
            else:
                i += 1
        elif arg.startswith("-MF"):
            # Handle -MF<file> without space - replace with our temp file
            transformed_args.append("-MF" + temp_depfile)
            i += 1
        else:
            transformed_args.append(arg)
            i += 1

    return output_file, transformed_args


def run_preprocessor(args: list[str]) -> bytes:
    """
    Run the preprocessor and return its output.

    Uses -o with a temporary file since some compiler wrappers suppress stdout.
    """
    with tempfile.NamedTemporaryFile(
        mode="wb", suffix=".i", delete=False
    ) as temp_output:
        temp_output_path = temp_output.name

    try:
        cmd = args + ["-o", temp_output_path]
        if DEBUG:
            print("command=" + shlex.join(cmd))
        subprocess.run(cmd, check=True)
        with open(temp_output_path, "rb") as f:
            return f.read()
    finally:
        try:
            os.unlink(temp_output_path)
        except OSError:
            pass


def compute_hash(data: bytes) -> str:
    """
    Compute SHA1 hash of the given data.
    """
    return hashlib.sha1(data).hexdigest()


if __name__ == "__main__":
    # Skip argv[0]
    args = sys.argv[1:]

    # Create a temporary file for the dependency output (currently we ignore it, but in
    # future we might want to examine the content for module deps)
    with tempfile.NamedTemporaryFile(
        mode="w", suffix=".d", delete=False
    ) as temp_depfile:
        temp_depfile_path = temp_depfile.name

    try:
        output_file, transformed_args = parse_and_transform_args(
            args, temp_depfile_path
        )

        if output_file is None:
            print("Error: No output file specified with -o", file=sys.stderr)
            sys.exit(1)

        try:
            preprocessor_output = run_preprocessor(transformed_args)
        except subprocess.CalledProcessError as e:
            print(
                f"Error: Preprocessor failed with exit code {e.returncode}",
                file=sys.stderr,
            )
            if e.stderr:
                sys.stderr.buffer.write(e.stderr)
            sys.exit(e.returncode)

        if DEBUG:
            print("contents=" + str(preprocessor_output))
        content_hash = compute_hash(preprocessor_output)

        with open(output_file, "w") as f:
            f.write(f"{content_hash}\n")

    finally:
        # Clean up temporary file
        try:
            os.unlink(temp_depfile_path)
        except OSError:
            pass

    sys.exit(0)
