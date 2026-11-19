#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import argparse
import json


def _dedupe_entries(entries: list) -> list:
    # swiftc rejects an explicit module map with two entries for the same module
    # name reached via distinct tset nodes. The dedup key is
    # (moduleName, is_swiftmodule); in the projected JSON, swiftmodule entries
    # have a modulePath field while clang pcm entries have a clangModulePath
    # field, so the presence of modulePath reproduces is_swiftmodule exactly.
    seen = set()
    deduped = []
    for entry in entries:
        if "modulePath" in entry:
            key = (entry["moduleName"], True)
        elif "clangModulePath" in entry:
            key = (entry["moduleName"], False)
        else:
            raise RuntimeError(f"Unrecognized module map entry: {entry}")

        if key in seen:
            continue
        seen.add(key)
        deduped.append(entry)

    return deduped


def main() -> None:
    parser = argparse.ArgumentParser(
        description="De-duplicate entries in a Swift explicit module map."
    )
    parser.add_argument(
        "--input",
        required=True,
        help="Path to the input Swift module map JSON.",
    )
    parser.add_argument(
        "--output",
        required=True,
        help="Path to write the de-duplicated Swift module map JSON.",
    )
    args = parser.parse_args()

    with open(args.input) as f:
        entries = json.load(f)

    deduped = _dedupe_entries(entries)

    with open(args.output, "w") as f:
        json.dump(deduped, f, indent=2)
        f.write("\n")


if __name__ == "__main__":
    main()
