# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Check that a JDK tool exists and comes from a JDK new enough for the prelude's Java tools.

`--tool` is what the toolchain runs: a path (`<java_home>/bin/javac`) or a name looked up on
`PATH`. If it is missing, or its JDK's `release` file names an older Java than `--java-version`,
print what to change and fail, rather than let a later action fail on a missing file or on
Java sources the JDK cannot compile. On success, `--output` becomes an empty stamp file.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import sys


def jdk_major_version(java_home: str) -> int | None:
    """The major version in `<java_home>/release` (8 for "1.8.0_402", 17 for "17.0.12")."""
    try:
        with open(os.path.join(java_home, "release"), encoding="utf-8") as f:
            m = re.search(r'^JAVA_VERSION="(?:1\.)?(\d+)', f.read(), re.MULTILINE)
    except OSError:
        return None
    return int(m.group(1)) if m else None


def missing_tool_message(tool: str, java_home: str, required: int) -> str:
    if java_home:
        return (
            f"`{tool}` not found. `[java] java_home` is `{java_home}`; set it in `.buckconfig` to the"
            + f" directory of a JDK {required} or newer (the directory that has `bin/java`)."
        )
    return (
        f"`{tool}` is not on `PATH`, and `[java] java_home` is not set. Install a JDK {required} or"
        + " newer, and set `[java] java_home` in `.buckconfig` to its directory or put its `bin`"
        + " directory on `PATH`."
    )


def old_jdk_message(path: str, java_home: str, major: int, required: int) -> str:
    lines = [
        f"`{path}` is from JDK {major} (`{java_home}`), but the prelude's Java tools are built for"
        + f" Java {required} (`[java] buck2_java_version`).",
        f"Use a JDK {required} or newer: set `[java] java_home` in `.buckconfig` to its directory.",
    ]
    if major >= 17:
        lines.append(
            f"Or build the tools for Java {major}: `-c java.buck2_java_version={major}`."
        )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tool", required=True)
    parser.add_argument("--java-home", default="", help="`[java] java_home`, if set")
    parser.add_argument("--java-version", type=int, required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()

    path = args.tool if os.sep in args.tool else shutil.which(args.tool)
    if not path or not os.path.isfile(path):
        print(
            missing_tool_message(args.tool, args.java_home, args.java_version),
            file=sys.stderr,
        )
        return 1

    java_home = os.path.dirname(os.path.dirname(os.path.realpath(path)))
    major = jdk_major_version(java_home)
    if major is not None and major < args.java_version:
        print(
            old_jdk_message(path, java_home, major, args.java_version), file=sys.stderr
        )
        return 1

    open(args.output, "w").close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
