#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


"""
Creates a Python Source DB JSON file from Python manifest JSON file (e.g. for use with Pyre).

Sources and dependencies are passed in via source manifest files, which are
merged by this script:

$ ./make_source_db_no_deps.py \
      my_rule.manifest.json \
      --output db_no_deps.json

The output format of the source DB is:

{
  <source1-name>: <source1-path>,
  <source2-name>: <source2-path>,
  ...
}
"""

import argparse
import json
import sys


def _load(path: str) -> list[tuple[str, str, str]]:
    with open(path) as f:
        return json.load(f)


def main(argv: list[str]) -> None:
    parser = argparse.ArgumentParser(fromfile_prefix_chars="@")
    parser.add_argument("--output", type=argparse.FileType("w"), default=sys.stdout)
    parser.add_argument("sources")
    args = parser.parse_args(argv[1:])

    sources = {name: path for name, path, _ in _load(args.sources)}
    json.dump(sources, args.output, indent=2)
    args.output.close()


sys.exit(main(sys.argv))
