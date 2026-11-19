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

from .signing_info import build_signing_info_from_file


def _main() -> None:
    parser = argparse.ArgumentParser(
        description="Tool which outputs signing-info JSON from a precomputed signing context. "
        "This is cheap (no profile directory scan, no security tool) and allows the "
        "[signing-info] subtarget to be built without running bundling.",
    )
    parser.add_argument(
        "--signing-context-path",
        type=Path,
        required=True,
        help="Path to precomputed signing context JSON.",
    )
    parser.add_argument(
        "--output",
        type=Path,
        required=True,
        help="Path to the output signing-info JSON file.",
    )

    args = parser.parse_args()

    signing_info = build_signing_info_from_file(args.signing_context_path)

    with open(args.output, "w") as f:
        json.dump(signing_info, f, indent=4)


if __name__ == "__main__":
    _main()
