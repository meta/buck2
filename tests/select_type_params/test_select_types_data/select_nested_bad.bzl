# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test cases that should FAIL type checking.
# Select with nested type parameters must reject mismatched plain values.

def accepts_select_list_str(x: Select[list[str]]) -> bool:
    """Only accepts select expressions, not plain lists."""
    return isinstance(x, Select)

def bad_call_plain_list():
    """ERROR: Passing a plain list to Select[list[str]] should fail."""
    accepts_select_list_str(["not", "a", "select"])

def bad_call_plain_str():
    """ERROR: Passing a plain string to Select[list[str]] should fail."""
    accepts_select_list_str("not a select")
