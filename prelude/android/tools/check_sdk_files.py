# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Check that files of the Android SDK exist before the build uses them.

Each `--file` is a path inside `--sdk-path`, paired with the `sdkmanager` package that ships it.
If one is missing, print what to change and fail, rather than let a later tool fail on a dangling
path. With `--link`, `--output` becomes a symlink to the only `--file`; otherwise it becomes an
empty stamp file.
"""

import argparse
import os
import sys


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk-path", required=True)
    parser.add_argument(
        "--file",
        nargs=2,
        action="append",
        metavar=("RELATIVE_PATH", "SDKMANAGER_PACKAGE"),
        required=True,
    )
    parser.add_argument("--link", action="store_true")
    parser.add_argument("--output", required=True)
    args = parser.parse_args()

    missing = [
        (os.path.join(args.sdk_path, relative_path), package)
        for relative_path, package in args.file
        if not os.path.exists(os.path.join(args.sdk_path, relative_path))
    ]
    if missing:
        lines = ["Android SDK files not found:"]
        lines += [f"  `{path}`" for path, _ in missing]
        lines += [
            "",
            f"`[android] sdk_path` is `{args.sdk_path}`. Point it at your Android SDK in `.buckconfig`"
            + " (usually the directory in `$ANDROID_HOME`), or install the missing packages:",
            "  sdkmanager "
            + " ".join(sorted({f'"{package}"' for _, package in missing})),
        ]
        print("\n".join(lines), file=sys.stderr)
        return 1

    if args.link:
        if len(args.file) != 1:
            parser.error("`--link` takes exactly one `--file`")
        os.symlink(os.path.join(args.sdk_path, args.file[0][0]), args.output)
    else:
        open(args.output, "w").close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
