# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

"""
Removes files from a framework that are not necessary when distributing
inside app bundles. Specifically Modules/* Headers/* and Documentation/*

Example Usage:
xcframework_sanitizer.py --input original/Foo.framework \
--output output/Foo.framework 
"""

import argparse
import os
import re
import shutil
from pathlib import Path
from typing import Callable, Iterable


def _should_ignore(
    framework_root: str,
) -> Callable[[str, list[str]], Iterable[str]]:
    prohibited: list[str] = ["Modules", "Headers", "Documentation"]

    def _should_ignore_impl(root: str, contents: list[str]) -> Iterable[str]:
        if re.sub(r"/Versions/[A-Z]", "", root) == framework_root:
            return prohibited
        return []

    return _should_ignore_impl


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Prepare a framework for distribution by removing unnecessary files."
    )

    parser.add_argument("--input", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--replacement-binary")
    args = parser.parse_args()

    out_path = Path(args.output)

    shutil.copytree(
        args.input,
        out_path,
        symlinks=True,
        dirs_exist_ok=False,
        ignore=_should_ignore(args.input),
    )

    if args.replacement_binary:
        framework_name = os.path.splitext(os.path.basename(out_path))[0]
        # Use realpath() because for macOS versioned bundles
        # we may need to follow a symlink:
        framework_binary_path = os.path.realpath(out_path / framework_name)
        os.chmod(framework_binary_path, 0o644)

        shutil.copy(args.replacement_binary, framework_binary_path)


if __name__ == "__main__":
    main()
