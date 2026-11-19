# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test cases that should FAIL type checking.
# Select[bool] must reject plain values that are not select expressions.

def accepts_select_bool(x: Select[bool]) -> bool:
    """Only accepts select expressions, not plain booleans."""
    return isinstance(x, Select)

def bad_call_plain_bool():
    """ERROR: Passing a plain bool to Select[bool] should fail."""
    accepts_select_bool(True)

def bad_call_plain_str():
    """ERROR: Passing a plain str to Select[bool] should fail."""
    accepts_select_bool("not a select")
