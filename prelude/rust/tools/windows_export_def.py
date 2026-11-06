#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Writes a module-definition file exporting every external symbol the given
# archives define, the way an ELF shared library exports every
# default-visibility symbol.

from __future__ import annotations

import argparse
import subprocess
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--archive", action="append", default=[])
    parser.add_argument("nm", nargs=argparse.REMAINDER)
    args = parser.parse_args()

    listing = subprocess.run(
        [*args.nm, "-A", "-P", "--defined-only", "--extern-only", *args.archive],
        check=True,
        capture_output=True,
        text=True,
    ).stdout

    exports = {}
    for line in listing.splitlines():
        member, symbol = line.split(": ", 1)
        name, kind = symbol.split(" ")[:2]
        # Members named after a DLL are the import library rustc embeds for
        # raw-dylib, and `__imp_` symbols are rustc's dllimport stubs.
        if member.endswith(".dll]") or name.startswith("__imp_"):
            continue
        if kind in ("T", "D", "R", "B"):
            exports[name] = kind != "T"

    with open(args.out, "w") as out:
        out.write("EXPORTS\n")
        for name, is_data in sorted(exports.items()):
            out.write(f'"{name}"{" DATA" if is_data else ""}\n')


if __name__ == "__main__":
    main()
