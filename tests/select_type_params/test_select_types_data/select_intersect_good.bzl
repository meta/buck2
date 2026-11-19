# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test that Select[T] intersection checking works correctly.
# Select[str] | Select[int] merges to Select[str | int] via union2.
# select() expressions with str or int branches should be accepted.

def accepts_select_str_or_int(x: Select[str] | Select[int]) -> bool:
    """Accepts select expressions resolving to str or int."""
    return isinstance(x, Select)

def good_with_select_str():
    """select() with str branches matches Select[str | int]."""
    accepts_select_str_or_int(select({"DEFAULT": "hello"}))

def good_with_select_int():
    """select() with int branches matches Select[str | int]."""
    accepts_select_str_or_int(select({"DEFAULT": 42}))
