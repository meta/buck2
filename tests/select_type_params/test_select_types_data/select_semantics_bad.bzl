# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test cases proving Select[str] != Select[str] | str.
# These calls should FAIL type checking because Select[str] alone
# does not accept a plain string value.

def strict_select_only(x: Select[str]) -> bool:
    """Only accepts select expressions, not plain strings."""
    return isinstance(x, Select)

def bad_call_strict_with_plain_str():
    """ERROR: Plain string to Select[str] should fail (need Select[str] | str for that)."""
    strict_select_only("not a select")

def bad_call_strict_with_plain_int():
    """ERROR: Plain int to Select[str] should also fail."""
    strict_select_only(42)
