# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import argparse
import json
import os
import pathlib
import sys


def merge_class_to_source_maps(mappings, output, relative_to=None):
    for mapping in mappings:
        with open(mapping) as f:
            obj = json.load(f)

        if relative_to is not None:
            obj["jarPath"] = os.path.relpath(obj["jarPath"], relative_to)
            for class_entry in obj["classes"]:
                if "srcPath" in class_entry:
                    class_entry["srcPath"] = os.path.relpath(
                        class_entry["srcPath"], relative_to
                    )

        json.dump(obj, output)
        print("", file=output)


def main(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--output", "-o", type=argparse.FileType("w"), default=sys.stdin
    )
    parser.add_argument(
        "--relative-to",
    )
    parser.add_argument("--mappings", "-m", type=pathlib.Path, required=True)
    args = parser.parse_args(argv[1:])

    with open(args.mappings) as f:
        mappings = [line.replace("\n", "") for line in f.readlines()]
    merge_class_to_source_maps(mappings, args.output, args.relative_to)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
