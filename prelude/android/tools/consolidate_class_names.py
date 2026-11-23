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

With --input-jars, each value is instead a list of jars (all jars of one target), and the output lists the names of
their `.class` entries without the suffix, in order.

With --class-names-mapping-file, a second JSON file maps target names to lists of class names files, such as the files
javacd and kotlincd write next to library jars. Their classes are added to the target's classes from
--input-mapping-file.

Output format (JSON):
{
    "target_name1": ["com/example/Class1", "com/example/Class2"],
    "target_name2": ["com/other/Class3"],
    ...
}
"""

import argparse
import json
import os
import zipfile
from pathlib import Path
from typing import Dict, List, Union


def _classpath_entries(path: str) -> List[str]:
    """Lists the entries of one jar as Java's ClasspathTraversal does: none for a missing path, the relative paths
    of all files under a directory, the entries of a file with a jar or zip extension (any case), and otherwise the
    file's name.

    Zip entry names are decoded as UTF-8, as Java's ZipFile does, also for entries without the UTF-8 flag
    (bit 11), which Python decodes as cp437."""
    if os.path.isdir(path):
        return sorted(
            os.path.relpath(os.path.join(root, name), path).replace(os.sep, "/")
            for root, _, names in os.walk(path, followlinks=True)
            for name in names
        )
    if not os.path.exists(path):
        return []
    if not path.lower().endswith((".jar", ".zip")):
        return [os.path.basename(path)]
    with zipfile.ZipFile(path) as jar:
        return [
            info.orig_filename
            if info.flag_bits & 0x800
            else info.orig_filename.encode("cp437").decode("utf-8")
            for info in jar.infolist()
        ]


def _read_class_names(value: Union[str, List[str]], input_jars: bool) -> List[str]:
    """Reads the classes of one target: from its class names file, or with `input_jars` from the `.class` entries of
    its jars, in order."""
    if isinstance(value, str) and not input_jars:
        with open(value, encoding="utf-8") as f:
            # Read class names, stripping whitespace
            return [line.strip() for line in f if line.strip()]
    if not (isinstance(value, list) and input_jars):
        raise SystemExit(
            f"Unexpected mapping value {value!r}: --input-jars takes lists of jars, otherwise paths"
        )
    names = [name for jar_path in value for name in _classpath_entries(jar_path)]
    return [name[: -len(".class")] for name in names if name.endswith(".class")]


def _read_targets(args: argparse.Namespace) -> Dict[str, List[str]]:
    """Reads the classes of each target from --input-mapping-file, then adds those of its class names files from
    --class-names-mapping-file."""
    with open(args.input_mapping_file) as f:
        target_to_file_map: Dict[str, Union[str, List[str]]] = json.load(f)
    targets = {
        target: _read_class_names(value, args.input_jars)
        for target, value in target_to_file_map.items()
    }
    if args.class_names_mapping_file:
        with open(args.class_names_mapping_file) as f:
            target_to_class_names_files: Dict[str, List[str]] = json.load(f)
        for target, paths in target_to_class_names_files.items():
            targets.setdefault(target, []).extend(
                name for path in paths for name in _read_class_names(path, False)
            )
    return targets


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
    parser.add_argument(
        "--skip-duplicate-check",
        action="store_true",
        help="Do not fail on classes that appear in more than one target of the batch.",
    )
    parser.add_argument(
        "--input-jars",
        action="store_true",
        help="The mapping values are lists of jars: list their `.class` entries.",
    )
    parser.add_argument(
        "--class-names-mapping-file",
        type=Path,
        help="JSON file mapping target names to lists of class names files, one class per line. Their classes are "
        "added to those the target gets from --input-mapping-file.",
    )
    args = parser.parse_args()

    # Read each class names file and build consolidated mapping
    # Also build reverse map to detect duplicates within this batch
    consolidated: Dict[str, List[str]] = {}
    class_to_target: Dict[str, str] = {}
    duplicates: Dict[str, List[str]] = {}

    for target_name, class_names in _read_targets(args).items():
        consolidated[target_name] = class_names
        if args.skip_duplicate_check:
            continue

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
