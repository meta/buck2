#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Creates a symlink tree for shared libraries (.so) and their debug files (.dwp).
# The input is an output directory to construct the symlink tree in and a JSON file of the shared libraries to link.
#
#
# Example:
# `shared_libraries_symlink_tree.py --shared_libs_symlink_tree /tmp/my_tree --shared_libraries_info_json /path/to/libraries.json``
#
# /path/to/libraries.json
# ```
# [
#     [
#         (
#             True,
#             "libthird-party_rust_vendor_anyhow_1.0.100.so",
#             "buck-out/v2/gen/fbsource/577ba3460b84e891/third-party/rust/vendor/anyhow/__1.0.100__/DPHL/libthird-party_rust_vendor_anyhow_1.0.100.so",
#             "buck-out/v2/gen/fbsource/577ba3460b84e891/third-party/rust/vendor/anyhow/__1.0.100__/DPHL/libthird-party_rust_vendor_anyhow_1.0.100.so.dwp",
#         ),
#         ....
#     ],
#     ...
# ]
# ```
#
# Each entry in the JSON file is a tuple of (is_str: bool, soname: str | Path, output: Path, dwp: Path | None)
# - is_str: whether the field `soname` is read literally as a string or if the field `soname` is a Path to a file containing the soname.
# - soname: The name of the link to the output. See its companion field `is_str`.
# - output: The Path to the .so. This is the target of the symlink.
# - dwp: An optional .dwp (DWARF Package) (i.e., debug info) file to symlink in a separate directory.
#
# Constructs symlink tree /tmp/my_tree
# ```
# libthird-party_rust_vendor_anyhow_1.0.100.so -> buck-out/v2/gen/fbsource/577ba3460b84e891/third-party/rust/vendor/anyhow/__1.0.100__/DPHL/libthird-party_rust_vendor_anyhow_1.0.100.so
# ...
# ```

import argparse
import json
import os
import sys
from pathlib import Path
from typing import IO, NamedTuple


class Args(NamedTuple):
    shared_libs_symlink_tree: Path
    dwp_symlink_tree: Path
    shared_libraries_info_json: IO[str]


def symlink_relative(link: Path, target: Path) -> None:
    """Create a symlink from link_name to target.

    The link is created by constructing a path to the target that is relative to link_name's parent.
    """
    symlink = os.path.relpath(target, start=os.path.dirname(link))
    os.symlink(symlink, link)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--shared_libs_symlink_tree",
        type=Path,
        required=True,
    )
    parser.add_argument(
        "--dwp_symlink_tree",
        type=Path,
        required=True,
    )
    parser.add_argument(
        "--shared_libraries_info_json",
        type=argparse.FileType(),
        required=True,
    )
    args = Args(**vars(parser.parse_args()))

    args.shared_libs_symlink_tree.mkdir(exist_ok=True)
    args.dwp_symlink_tree.mkdir(exist_ok=True)

    shared_libraries_info = json.load(args.shared_libraries_info_json)

    seen: dict[str, str] = {}

    for shared_libraries in shared_libraries_info:
        for shared_library in shared_libraries:
            is_str, soname, output, dwp = shared_library
            if is_str:
                link_name = soname
            else:
                link_name = Path(soname).read_text().strip()

            existing = seen.get(link_name)
            if existing is not None:
                if existing != output:
                    print(
                        f"Warning: Duplicate shared library {link_name}:\n"
                        f"  first:  {existing}\n"
                        f"  second: {output}\n"
                        f"  Using the first one.",
                        file=sys.stderr,
                    )
                continue

            seen[link_name] = output
            target = output

            symlink_relative(args.shared_libs_symlink_tree / link_name, target)

            has_dwp = dwp is not None
            if has_dwp:
                symlink_relative(
                    link=args.dwp_symlink_tree / (link_name + ".dwp"),
                    target=dwp,
                )


if __name__ == "__main__":
    main()
