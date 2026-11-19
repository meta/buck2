#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import sys
from pathlib import Path


def main() -> None:
    print(Path(sys.argv[1]).read_text())
    if len(sys.argv) == 3:
        Path(sys.argv[2]).touch()
    sys.exit(1)


if __name__ == "__main__":
    main()
