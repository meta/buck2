# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test cases that should FAIL type checking.
# Select[str | None] must reject plain values that are not select expressions.
# Select[str | None] is distinct from Select[str]: the latter rejects None values.

def accepts_select_str_or_none(x: Select[str | None]) -> bool:
    """Only accepts select expressions, not plain strings or None."""
    return isinstance(x, Select)

def bad_call_plain_str():
    """ERROR: Passing a plain string to Select[str | None] should fail."""
    accepts_select_str_or_none("not a select")

def bad_call_plain_none():
    """ERROR: Passing plain None to Select[str | None] should fail."""
    accepts_select_str_or_none(None)
