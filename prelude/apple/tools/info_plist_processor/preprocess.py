# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import json
import re
import sys
from enum import Enum
from typing import Optional, TextIO


class _ReGroupName(str, Enum):
    openparen = "openparen"
    variable = "variable"
    modifier = "modifier"
    closeparen = "closeparen"

    def __format__(self, spec):
        return f"{self.value}"


_re_string: str = "\\$(?P<{openparen}>[\\{{\\(])(?P<{variable}>[^\\}}\\):]+)(?::(?P<{modifier}>[^\\}}\\)]+))?(?P<{closeparen}>[\\}}\\)])".format(
    openparen=_ReGroupName.openparen,
    variable=_ReGroupName.variable,
    modifier=_ReGroupName.modifier,
    closeparen=_ReGroupName.closeparen,
)


def _make_substitution_dict(
    substitutions_json_file: TextIO, product_name: str
) -> dict[str, str]:
    result = {
        "EXECUTABLE_NAME": product_name,
        "PRODUCT_NAME": product_name,
    }
    if substitutions_json_file is not None:
        # JSON file take precedence over default substitutions
        result.update(json.load(substitutions_json_file))
    return result


def _process_line(
    line: str,
    pattern: re.Pattern[str],
    substitutions: dict[str, str],
    minimum_os_version_key: Optional[str],
) -> str:
    if (
        minimum_os_version_key is not None
        and line.strip() == f"<key>{minimum_os_version_key}</key>"
    ):
        print("Found minimum OS version key in plist file", file=sys.stderr)
        sys.exit(1)

    return _expand(line, pattern, substitutions, [])


def _expand(
    text: str,
    pattern: re.Pattern[str],
    substitutions: dict[str, str],
    expanding: list[str],
) -> str:
    # `expanding` lists the variables whose values are being expanded, outermost first, so a
    # variable that shows up inside its own value is a cycle.
    result = []
    pos = 0
    while True:
        match = pattern.search(text, pos)
        if match is None:
            result.append(text[pos:])
            return "".join(result)
        result.append(text[pos : match.start()])
        key = match.group(_ReGroupName.variable)
        if key in substitutions:
            if key in expanding:
                raise RuntimeError(
                    "Recursive plist variable: " + " -> ".join(expanding + [key])
                )
            expanding.append(key)
            result.append(
                _expand(substitutions[key], pattern, substitutions, expanding)
            )
            expanding.pop()
        else:
            result.append(match.group(0))
        pos = match.end()


def preprocess(
    input_file: TextIO,
    output_file: TextIO,
    substitutions_file: TextIO,
    product_name: str,
    minimum_os_version_key: Optional[str] = None,
) -> None:
    pattern = re.compile(_re_string)
    substitutions = _make_substitution_dict(substitutions_file, product_name)
    for line in input_file:
        output_file.write(
            _process_line(line, pattern, substitutions, minimum_os_version_key)
        )
