# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test cases that should FAIL type checking.
# Select[str] must reject a plain str argument.

def accepts_select_only(x: Select[str]) -> bool:
    """Only accepts select expressions, not plain strings."""
    return isinstance(x, Select)

def bad_call_plain_str():
    """ERROR: Passing a plain string to Select[str] should fail."""
    accepts_select_only("not a select")

def bad_call_plain_int():
    """ERROR: Passing a plain int to Select[str] should also fail."""
    accepts_select_only(42)
