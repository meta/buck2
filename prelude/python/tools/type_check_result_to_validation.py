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
from pathlib import Path
from typing import Any


def convert_type_check_result(input_path: Path, output_path: Path) -> None:
    """Convert a Python type-check result to Buck2's ValidationSpec format."""
    validation: dict[str, object] = {
        "version": 1,
        "data": {"status": "success"},
    }

    try:
        with input_path.open() as input_file:
            data: Any = json.load(input_file)
    except (OSError, json.JSONDecodeError):
        validation["data"] = {
            "status": "failure",
            "message": "Failed to read type checker output",
        }
    else:
        messages = []
        malformed = False
        errors = data.get("errors", []) if isinstance(data, dict) else None
        if not isinstance(errors, list):
            errors = []
            malformed = True

        for error in errors:
            if not isinstance(error, dict) or "code" not in error:
                malformed = True
                continue
            if (
                error["code"] == 0
                or error.get("name") in ("unused-ignore", "unused-type-ignore")
                or error.get("severity") in ("info", "ignore", "warn")
            ):
                continue
            try:
                messages.append(
                    f"{error['path']}:{error['line']}:{error['column']} {error['description']}"
                )
            except KeyError:
                malformed = True

        if malformed:
            messages.append("Malformed type checker output")
        if messages:
            validation["data"] = {
                "status": "failure",
                "message": "\n".join(messages),
            }

    with output_path.open("w") as output_file:
        json.dump(validation, output_file)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("input", type=Path, help="Path to type_check_result.json")
    parser.add_argument(
        "--output",
        type=Path,
        required=True,
        help="Path to write ValidationSpec JSON",
    )
    args = parser.parse_args()
    convert_type_check_result(args.input, args.output)


if __name__ == "__main__":
    main()
