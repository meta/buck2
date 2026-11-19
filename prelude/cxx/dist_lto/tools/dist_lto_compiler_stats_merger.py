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
import sys

"""
A helper script designed to merge multiple JSON files storing LLVM stats produced by -stats -stats-json -info-out-file=. All stats are summed together, to form a new JSON file.
"""


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--input-stats-filelist",
        help="A filelist of JSON static files produced by LLVM's -stats -stats-json",
    )
    parser.add_argument(
        "--output-stats-file", help="A path to write the merged JSON static file to."
    )
    args = parser.parse_args(argv[1:])
    merged_stats: dict[int, str] = {}
    with open(args.input_stats_filelist) as stats_filelist:
        for stats_file in stats_filelist:
            with open(stats_file.strip()) as f:
                stats = json.load(f)
                for key, value in stats.items():
                    if key in merged_stats:
                        merged_stats[key] += value
                    else:
                        merged_stats[key] = value

    with open(args.output_stats_file, "w") as f:
        json.dump(merged_stats, f, sort_keys=True, indent=4)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
