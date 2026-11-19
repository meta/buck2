# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import sys


def main() -> None:
    with open(sys.argv[1], "rb") as src:
        content = src.read()
    with open(sys.argv[2], "wb") as dst:
        dst.write(content)


if __name__ == "__main__":
    main()
