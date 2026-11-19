#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import argparse
import subprocess
import sys


def main(argv: list[str]) -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--patchelf", action="append", required=True)
    parser.add_argument("-f", action="store_true")
    parser.add_argument("--rpath", action="append", dest="rpaths", default=[])
    parser.add_argument("-o", "--output", required=True)
    parser.add_argument("path")
    args = parser.parse_args(argv[1:])

    if args.patchelf is None:
        args.patchelf = ["patchelf"]

    if not args.rpaths:
        parser.error("no rpaths specified")

    # If this path isn't an ELF file, then there's nothing to do.
    with open(args.path, "rb") as f:
        if f.read(4) != b"\x7fELF":
            # do a reflink copy from path to output
            subprocess.check_call(["cp", "--reflink=auto", args.path, args.output])
            return

    # Otherwise, patch in the rpaths.
    subprocess.check_call(
        args.patchelf
        + [
            "--force-rpath",
            "--add-rpath",
            ":".join(args.rpaths),
            "--output",
            args.output,
            args.path,
        ]
    )


if __name__ == "__main__":
    main(sys.argv)
