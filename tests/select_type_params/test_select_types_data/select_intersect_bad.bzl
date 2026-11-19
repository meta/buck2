# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test that Select[T] intersection rejects non-overlapping types.
# Select[str] | Select[int] merges to Select[str | int].
# select() with bool branches and plain values should be rejected.

def accepts_select_str_or_int(x: Select[str] | Select[int]) -> bool:
    """Accepts select expressions resolving to str or int."""
    return isinstance(x, Select)

def bad_with_select_bool():
    """ERROR: Select[bool] does not intersect Select[str | int]."""
    accepts_select_str_or_int(select({"DEFAULT": True}))

def bad_with_plain_str():
    """ERROR: Plain str does not intersect Select[str | int]."""
    accepts_select_str_or_int("hello")
