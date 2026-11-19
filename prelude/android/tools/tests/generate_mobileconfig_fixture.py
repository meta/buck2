# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Capture a raw D8 ref-count file and its reviewed planner expectations together.

From this directory, regenerate the pinned MobileConfig fixture with:
  fbpython generate_mobileconfig_fixture.py \
    --ref-count-path testdata/mobileconfig_definition_lib_ref_count.txt \
    --weight-estimate 8232224 --expected-group-sizes 17 10 1 \
    --source-revision b53bf164

The pinned build's weight ratio is 8232224 / 5242880, about 1.57, so its
28 classes first form runs of 17 and 11. The first run fits. The other has
65560 fields and needs the reviewed split of 10 classes and one final class.
These boundaries are inputs, not calculated by the planner under test.

For a new snapshot, build fb4a's arm64 debug MobileConfig definitions jar and
replay its pre_dex command with --class-ref-counts. Use its --ref-count-path
output and --weight-estimate-path value from the SAME invocation. Pass the
source revision and the independently reviewed group sizes below. Never use
merged APK dex counts or the planner under test to generate expectations.
"""

import argparse
import json
import pathlib

# fb4a uses [android] secondary_dex_weight_limit = 5242880 from
# tools/buckconfigs/fbsource-mobile-common.bcfg, not Buck's 12 MiB fallback.
_FB4A_SECONDARY_DEX_WEIGHT_LIMIT = 5 * 1024 * 1024
_FIXTURE_NAME = "mobileconfig_definition_lib_ref_count"


def fixture_expectations(
    ref_counts: str,
    weight_estimate: int,
    group_sizes: list[int],
    source_revision: str,
) -> dict:
    """Sum raw counts over explicit, reviewed groups without calling the planner."""
    _, *class_lines = ref_counts.strip().splitlines()
    counts = {}
    for line in class_lines:
        methods, fields, types, name = line.split()
        counts[name + ".class"] = [int(methods), int(fields), int(types)]
    classes = sorted(counts, key=lambda name: name.removesuffix(".class"))
    if any(size <= 0 for size in group_sizes) or sum(group_sizes) != len(classes):
        raise ValueError("Expected group sizes must cover every class exactly once")
    groups = []
    start = 0
    for size in group_sizes:
        names = classes[start : start + size]
        groups.append(
            {
                "class_names": names,
                "ref_counts": [
                    sum(counts[name][i] for name in names) for i in range(3)
                ],
            }
        )
        start += size
    return {
        "source_revision": source_revision,
        "weight_estimate": weight_estimate,
        "secondary_dex_weight_limit": _FB4A_SECONDARY_DEX_WEIGHT_LIMIT,
        "expected_groups": groups,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ref-count-path", type=pathlib.Path, required=True)
    parser.add_argument("--weight-estimate", type=int, required=True)
    parser.add_argument("--expected-group-sizes", type=int, nargs="+", required=True)
    parser.add_argument("--source-revision", required=True)
    parser.add_argument(
        "--output-dir",
        type=pathlib.Path,
        default=pathlib.Path(__file__).parent / "testdata",
    )
    args = parser.parse_args()
    ref_counts = args.ref_count_path.read_text()
    expectations = fixture_expectations(
        ref_counts,
        args.weight_estimate,
        args.expected_group_sizes,
        args.source_revision,
    )
    args.output_dir.mkdir(parents=True, exist_ok=True)
    (args.output_dir / f"{_FIXTURE_NAME}.txt").write_text(ref_counts)
    (args.output_dir / f"{_FIXTURE_NAME}.json").write_text(
        json.dumps(expectations, indent=2) + "\n"
    )


if __name__ == "__main__":
    main()
