# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Extract the ProGuard consumer rules a library jar ships for its users.

Library jars carry them under `META-INF/com.android.tools/<shrinker>/` or, in the legacy
layout, `META-INF/proguard/`. As in the Android Gradle Plugin, a jar that has any
`META-INF/com.android.tools/` entry provides only its `proguard/` files, otherwise its
`META-INF/proguard/` files. They are concatenated into one output, empty when there are none.
"""

import argparse
import zipfile

_ANDROID_TOOLS = "META-INF/com.android.tools/"
_PROGUARD_SPECIFIC = _ANDROID_TOOLS + "proguard/"
_GENERIC = "META-INF/proguard/"


def extract(jar: str, output: str) -> None:
    with zipfile.ZipFile(jar) as z:
        names = sorted(z.namelist())
        prefix = (
            _PROGUARD_SPECIFIC
            if any(n.startswith(_ANDROID_TOOLS) for n in names)
            else _GENERIC
        )
        selected = [n for n in names if n.startswith(prefix) and not n.endswith("/")]
        with open(output, "wb") as out:
            for name in selected:
                out.write(f"# From {name}\n".encode())
                out.write(z.read(name))
                out.write(b"\n")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--jar", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    extract(args.jar, args.output)


if __name__ == "__main__":
    main()
