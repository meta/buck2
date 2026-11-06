#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import argparse
import json
import sys

from .dsym_allowlist import (
    kept_debug_file_paths,
    linked_target_output_paths,
    write_dsymutil_allowlist,
)


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Writes a `dsymutil --allow` file listing the debug map entries of an executable/dylib that the scrubber would keep, or the targets in its debug map. The input is not modified."
    )
    parser.add_argument(
        "--input",
        required=True,
        help="Path to the input which is an executable/dylib file.",
    )
    parser.add_argument("--output", help="Path to the YAML file to write.")
    parser.add_argument(
        "--linked-targets-output",
        help="Instead of the YAML file, write a JSON file with the output directories of the targets in the debug map of the input.",
    )
    group = parser.add_mutually_exclusive_group()
    group.add_argument(
        "--targets-file",
        help="Path to a json file which contains user-focused Buck targets",
    )
    group.add_argument(
        "--spec-file",
        help="Path to a json file which contains user-focused include/exclude specs",
    )
    parser.add_argument(
        "--persisted-targets-file",
        help="A JSON file with additional targets that must be preserved by the scrubber.",
    )
    args = parser.parse_args()
    if args.linked_targets_output is not None:
        if (
            args.output
            or args.targets_file
            or args.spec_file
            or args.persisted_targets_file
        ):
            parser.error("--linked-targets-output takes only --input")
    elif args.output is None or (args.targets_file is None and args.spec_file is None):
        parser.error(
            "--output and one of --targets-file or --spec-file are required without --linked-targets-output"
        )
    return args


def main() -> None:
    args = _parse_args()
    try:
        if args.linked_targets_output:
            with open(args.linked_targets_output, "w") as f:
                json.dump(
                    {"output_paths": linked_target_output_paths(args.input)},
                    f,
                    indent=2,
                )
                f.write("\n")
            return
        paths = kept_debug_file_paths(
            input_file=args.input,
            persisted_targets_file=args.persisted_targets_file,
            targets_file=args.targets_file,
            spec_file=args.spec_file,
        )
        write_dsymutil_allowlist(paths, args.output)
    except Exception as e:
        print(f"Focused debugging failed: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
