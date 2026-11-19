# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test that Select[str] | list[str] rejects mismatched types.

def accepts_select_or_list(x: Select[str] | list[str]) -> bool:
    """Accepts either a select expression or a list of strings."""
    return isinstance(x, Select)

def bad_plain_int():
    """ERROR: Plain int should not match Select[str] | list[str]."""
    accepts_select_or_list(42)

def bad_plain_str():
    """ERROR: Plain str should not match Select[str] | list[str]."""
    accepts_select_or_list("hello")
