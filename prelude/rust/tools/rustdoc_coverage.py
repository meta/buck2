#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import argparse
import json
import re
import subprocess
from pathlib import Path
from typing import NamedTuple


class Args(NamedTuple):
    out: Path
    cmd: list[str]


def arg_parse() -> Args:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "out",
        type=Path,
        help="path to output",
    )
    parser.add_argument("cmd", nargs=argparse.REMAINDER, help="command to run")
    return Args(**vars(parser.parse_args()))


_REGEX = re.compile(r"(\d+(?:\.\d+)?)")


def main():
    args = arg_parse()
    stdout = subprocess.check_output(args.cmd, text=True)

    with open(args.out, "w") as f:
        # not using json output until https://github.com/rust-lang/rust/issues/117291 is fixed
        # stdout looks like...
        # +--------+------------+------------+------------+------------+
        # | File   | Documented | Percentage |   Examples | Percentage |
        # +--------+------------+------------+------------+------------+
        # | foo.rs |          1 |       1.0% |          0 |       0.0% |
        # | bar.rs |          2 |       2.1% |          0 |       0.0% |
        # +--------+------------+------------+------------+------------+
        # | Total  |          3 |       3.1% |          0 |       0.0% |
        # +--------+------------+------------+------------+------------+
        total_line = stdout.splitlines()[-2]
        nums = _REGEX.findall(total_line)
        if len(nums) != 4:
            raise Exception(
                f"using regex `{_REGEX.pattern}`, expected to find 4 numbers, got {len(nums)} "
                f"for line: '{total_line}'"
            )
        json.dump(
            {
                "documented": nums[0],
                "documented_percentage": nums[1],
                "examples": nums[2],
                "examples_percentage": nums[3],
            },
            f,
        )


if __name__ == "__main__":
    main()
