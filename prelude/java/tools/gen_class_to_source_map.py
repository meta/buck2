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
import os
import sys
from collections import defaultdict
from collections.abc import Iterable
from typing import TextIO

from class_to_source_map import build_class_entries, write_sources_jar


def _base_class_name_matches_base_source_path(
    base_class_name: str, base_source_path: str
) -> bool:
    return base_class_name == base_source_path or base_source_path.endswith(
        "/" + base_class_name
    )


def _load_debug_class_to_source_map(path: str | None) -> dict[str, str]:
    if path is None:
        return {}

    with open(path) as debuginfo_file:
        source_files = json.load(debuginfo_file)

    class_to_sources: defaultdict[str, set[str]] = defaultdict(set)
    for source_file in source_files:
        for class_info in source_file["classes"]:
            class_to_sources[class_info["name"]].add(source_file["file_path"])

    unique_class_to_source: dict[str, str] = {}
    for class_name, source_paths in class_to_sources.items():
        # Never guess which source owns a class claimed by multiple files.
        if len(source_paths) != 1:
            continue
        unique_class_to_source[class_name] = next(iter(source_paths))
    return unique_class_to_source


def _legacy_source_for_class(class_name: str, sources: dict[str, str]) -> str | None:
    base_class_name = class_name.replace(".", "/")
    for source_base, source_path in sources.items():
        if _base_class_name_matches_base_source_path(base_class_name, source_base):
            return source_path
    # Only a class with no source of its own is a Kotlin file facade (`Foo.kt` -> `FooKt`).
    if base_class_name.endswith("Kt"):
        for source_base, source_path in sources.items():
            if _base_class_name_matches_base_source_path(
                base_class_name[:-2], source_base
            ):
                return source_path
    return None


def _resolve_source_path(
    compiled_class: str,
    top_level_class: str,
    debug_class_to_source: dict[str, str],
    legacy_sources: dict[str, str],
) -> str | None:
    # Debug records are authoritative. Legacy matching keeps toolchains without
    # debug support, and classes missing from otherwise valid debug data, working.
    return (
        debug_class_to_source.get(compiled_class)
        or debug_class_to_source.get(top_level_class)
        or _legacy_source_for_class(top_level_class, legacy_sources)
    )


def _build_class_entries(
    jar: str,
    source_paths: Iterable[str],
    debuginfo: str | None,
    include_classes_prefixes: Iterable[str],
) -> list[dict[str, str]]:
    legacy_sources = {os.path.splitext(source)[0]: source for source in source_paths}
    debug_class_to_source = _load_debug_class_to_source_map(debuginfo)
    return build_class_entries(
        jar,
        lambda compiled_class, top_level_class: _resolve_source_path(
            compiled_class, top_level_class, debug_class_to_source, legacy_sources
        ),
        include_classes_prefixes,
    )


def generate_class_to_source_map(
    jar: str,
    source_paths: Iterable[str],
    output: TextIO,
    *,
    debuginfo: str | None = None,
    include_classes_prefixes: Iterable[str] = (),
    owner_target: str | None = None,
    sources_jar: str | None = None,
) -> None:
    classes = _build_class_entries(
        jar,
        source_paths,
        debuginfo,
        include_classes_prefixes,
    )
    if sources_jar is not None:
        write_sources_jar(classes, sources_jar)

    result = {"jarPath": jar, "classes": classes}
    if owner_target is not None:
        result["ownerTarget"] = owner_target
    json.dump(result, output)
    output.write("\n")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(fromfile_prefix_chars="@")
    parser.add_argument(
        "--include_classes_prefixes",
        "-i",
        default=[],
        nargs="*",
        action="extend",
        help="Prefixes of classes to include in the output, even if their source isn't present",
    )
    parser.add_argument(
        "--output", "-o", type=argparse.FileType("w"), default=sys.stdout
    )
    parser.add_argument("--sources_jar", required=False)
    parser.add_argument("--debuginfo", required=False)
    parser.add_argument("--owner-target", required=False)
    parser.add_argument("jar")
    parser.add_argument("sources", nargs="*")
    args = parser.parse_args(argv[1:])

    generate_class_to_source_map(
        args.jar,
        args.sources,
        args.output,
        debuginfo=args.debuginfo,
        include_classes_prefixes=args.include_classes_prefixes,
        owner_target=args.owner_target,
        sources_jar=args.sources_jar,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
