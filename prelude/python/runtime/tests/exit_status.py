# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

"""`main_function`s whose return value is chosen by `argv[1]`, to observe as exit statuses."""

import sys

RETURN_VALUES: dict[str, object] = {
    "none": None,
    "zero": 0,
    "three": 3,
    "message": "it broke",
}


def main() -> object:
    if sys.argv[1] == "sys-exit":
        sys.exit(5)
    return RETURN_VALUES[sys.argv[1]]


async def async_main() -> object:
    return main()
