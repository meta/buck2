# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Consolidates multiple class name files into a single JSON file.

This tool reads a mapping of target names to class name file paths,
reads each file, and outputs a consolidated JSON containing all
class names directly. This reduces the number of file reads required
by downstream tools.

Additionally, this tool builds a reverse map (class -> target) to detect
duplicate classes within the batch. If duplicates are found, it fails fast
with an error message. This allows earlier detection of duplicate classes
compared to waiting for the global duplicate_class_checker.py to run.

Input format (JSON):
{
    "target_name1": "path/to/class_names1.txt",
    "target_name2": "path/to/class_names2.txt",
    ...
}

Output format (JSON):
{
    "target_name1": ["com/example/Class1", "com/example/Class2"],
    "target_name2": ["com/other/Class3"],
    ...
}
"""

import argparse
import json
from pathlib import Path
from typing import Dict, List


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Consolidate multiple class name files into a single JSON file."
    )
    parser.add_argument(
        "--input-mapping-file",
        type=Path,
        required=True,
        help="JSON file mapping target names to class name file paths.",
    )
    parser.add_argument(
        "--output-file",
        type=Path,
        required=True,
        help="Output JSON file with consolidated class names.",
    )
    args = parser.parse_args()

    # Read the input mapping
    with open(args.input_mapping_file) as f:
        target_to_file_map: Dict[str, str] = json.load(f)

    # Read each class names file and build consolidated mapping
    # Also build reverse map to detect duplicates within this batch
    consolidated: Dict[str, List[str]] = {}
    class_to_target: Dict[str, str] = {}
    duplicates: Dict[str, List[str]] = {}

    for target_name, file_path in target_to_file_map.items():
        with open(file_path) as f:
            # Read class names, stripping whitespace
            class_names = [line.strip() for line in f if line.strip()]
            consolidated[target_name] = class_names

            # Check for duplicates within this batch
            # Skip inner classes and module-info (same filtering as duplicate_class_checker.py)
            for class_name in class_names:
                if "$" in class_name or "module-info" in class_name:
                    continue
                if class_name in class_to_target:
                    # Found a duplicate
                    if class_name not in duplicates:
                        duplicates[class_name] = [class_to_target[class_name]]
                    duplicates[class_name].append(target_name)
                else:
                    class_to_target[class_name] = target_name

    # Fail fast if duplicates found within this batch
    if duplicates:
        error_lines = ["Duplicate class names found within batch:"]
        for class_name, targets in sorted(duplicates.items()):
            error_lines.append(f"  {class_name} exists in: {', '.join(targets)}")
        raise SystemExit("\n".join(error_lines))

    # Write consolidated output
    with open(args.output_file, "w") as f:
        json.dump(consolidated, f)


if __name__ == "__main__":
    main()
