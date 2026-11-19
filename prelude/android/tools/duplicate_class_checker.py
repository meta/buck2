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
import zipfile
from pathlib import Path
from typing import Dict, List, Set


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Check whether the provided list of files contains any duplicate class names in them."
    )
    parser.add_argument(
        "--jar-to-owning-target-map-file",
        type=Path,
        help="A file that contains the mapping between jar paths and the owning target.",
        required=False,
    )
    parser.add_argument(
        "--validation-output",
        type=Path,
        required=True,
    )
    parser.add_argument(
        "--consolidated-files-list",
        type=Path,
        help="A file containing a list of consolidated JSON files (one path per line).",
        required=False,
    )
    parser.add_argument(
        "--mode",
        type=str,
        choices=["pre-dexed-libs", "non-pre-dexed-jars"],
        required=True,
    )
    args = parser.parse_args()

    if args.mode == "pre-dexed-libs":
        class_to_targets_map = get_class_to_target_mapping_from_consolidated_files(
            args.consolidated_files_list
        )
    elif args.mode == "non-pre-dexed-jars":
        class_to_targets_map = get_class_to_target_mapping_from_jars(
            args.jar_to_owning_target_map_file
        )
    else:
        raise ValueError(f"Unknown mode: {args.mode}")

    duplicate_classes = {
        class_name: target_name_list
        for class_name, target_name_list in class_to_targets_map.items()
        if len(target_name_list) > 1
    }

    print(build_validation_message(duplicate_classes), file=sys.stderr)
    if duplicate_classes:
        sys.exit(1)
    else:
        with open(args.validation_output, "w") as f:
            f.write("No duplicate class names found")
        sys.exit(0)


def get_class_to_target_mapping_from_consolidated_files(
    consolidated_files_list: Path,
) -> Dict[str, List[str]]:
    """
    Reads consolidated JSON files and builds a mapping from class names to targets.

    Each consolidated file is a JSON object mapping target names to lists of class names.
    This is much faster than reading individual files because:
    1. Fewer files to open (32 batches instead of 40K+ files)
    2. Consolidated files are already in the optimal format

    Note: Batch-level duplicate detection is already done in consolidate_class_names.py,
    so duplicates within a single batch will fail fast at consolidation time.
    This function only needs to detect cross-batch duplicates.
    """
    # Read the list of consolidated files
    with open(consolidated_files_list) as f:
        consolidated_files = [line.strip() for line in f if line.strip()]

    class_to_target_mapping: Dict[str, List[str]] = {}

    # Read each consolidated file and merge the results
    for consolidated_file in consolidated_files:
        with open(consolidated_file) as f:
            target_to_classes: Dict[str, List[str]] = json.load(f)

        for target_name, class_names in target_to_classes.items():
            for class_name in class_names:
                if "$" not in class_name and "module-info" not in class_name:
                    class_to_target_mapping.setdefault(class_name, []).append(
                        target_name
                    )

    return class_to_target_mapping


def get_class_to_target_mapping_from_jars(
    jar_to_owning_target_map_file: Path,
) -> Dict[str, List[str]]:
    """
    Reads a JSON file that maps JAR file paths to their owning targets,
    extracts class names from each JAR file, and builds a mapping
    from class names to the targets that contain them.
    """
    with open(jar_to_owning_target_map_file) as f:
        target_to_jar_file_map = json.loads(f.read())

    class_to_target_mapping: Dict[str, List[str]] = {}
    for jar_path, target_name in target_to_jar_file_map.items():
        class_names = extract_class_names_from_jar(jar_path)
        for class_name in class_names:
            if "$" in class_name or "module-info" in class_name:
                continue
            class_to_target_mapping.setdefault(class_name, []).append(target_name)

    return class_to_target_mapping


def extract_class_names_from_jar(jar_path: str) -> Set[str]:
    """
    Extracts class names from a JAR file.
    Returns a set of fully qualified class names.
    """
    class_names = set()
    with zipfile.ZipFile(jar_path, "r") as jar:
        for entry in jar.namelist():
            if entry.endswith(".class"):
                # Convert path/to/ClassName.class to path.to.ClassName
                class_name = entry[: -len(".class")].replace("/", ".")
                class_names.add(class_name)
    return class_names


def build_validation_message(
    duplicate_classes: Dict[str, List[str]],
) -> str:
    if not duplicate_classes:
        return "No duplicate class names found"

    messages = []
    for class_name, targets in duplicate_classes.items():
        targets_str = ", ".join(sorted(targets))
        messages.append(
            f"* {class_name} exists in the following targets: {targets_str}\n",
        )
    return f"""
Duplicate class name(s) found:

This means multiple copies of the same class is being included in the final apk.
Check the class names and the target names listed below and make sure to only include one copy of each class.
Run `buck cquery allpaths(<apk_target>, <library_target>)` or `arc deppy explain-dep <apk_target> <library_target>` to find the dependency paths.
{"".join(messages)}
        """


if __name__ == "__main__":
    main()
