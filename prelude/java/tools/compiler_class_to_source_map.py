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
from collections.abc import Iterable
from typing import TextIO

from class_to_source_map import build_class_entries, write_sources_jar


def generate_class_to_source_map(
    jar: str,
    compiler_map: str,
    source_paths: Iterable[str],
    output: TextIO,
    *,
    include_classes_prefixes: Iterable[str] = (),
    owner_target: str | None = None,
    sources_jar: str | None = None,
) -> None:
    with open(compiler_map) as mapping_file:
        mapping: dict[str, list[str]] = json.load(mapping_file)
    sources = {os.path.normpath(path).replace("\\", "/"): path for path in source_paths}

    def source_for_class(compiled_class: str, _top_level_class: str) -> str | None:
        origins = mapping.get(compiled_class.replace(".", "/") + ".class", [])
        candidates = {
            sources[path]
            for origin in origins
            if (path := os.path.normpath(origin).replace("\\", "/")) in sources
        }
        return next(iter(candidates)) if len(candidates) == 1 else None

    classes = build_class_entries(jar, source_for_class, include_classes_prefixes)
    if sources_jar is not None:
        write_sources_jar(classes, sources_jar)
    result = {"jarPath": jar, "classes": classes}
    if owner_target is not None:
        result["ownerTarget"] = owner_target
    json.dump(result, output)
    output.write("\n")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(fromfile_prefix_chars="@")
    parser.add_argument("--compiler-map", required=True)
    parser.add_argument(
        "--include_classes_prefixes", "-i", nargs="*", action="extend", default=[]
    )
    parser.add_argument(
        "--output", "-o", type=argparse.FileType("w"), default=sys.stdout
    )
    parser.add_argument("--owner-target")
    parser.add_argument("--sources_jar")
    parser.add_argument("jar")
    parser.add_argument("sources", nargs="*")
    args = parser.parse_args(argv[1:])
    generate_class_to_source_map(
        args.jar,
        args.compiler_map,
        args.sources,
        args.output,
        include_classes_prefixes=args.include_classes_prefixes,
        owner_target=args.owner_target,
        sources_jar=args.sources_jar,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
