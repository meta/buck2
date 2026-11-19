# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test cases for type checking of select() branch values.
# When a function expects Select[str], passing select({"DEFAULT": 42}) should fail
# because the branch value (int) does not match the expected parameter type (str).
# All functions here should pass type checking.

def accepts_select_str(x: Select[str]) -> bool:
    """Expects Select[str]."""
    return isinstance(x, Select)

def call_with_matching_str_branches():
    """All branches are str, which matches Select[str]."""
    accepts_select_str(
        select({
            "DEFAULT": "default_value",
            "config//os:linux": "linux_value",
        })
    )

def accepts_select_int(x: Select[int]) -> bool:
    """Expects Select[int]."""
    return isinstance(x, Select)

def call_with_matching_int_branches():
    """All branches are int, which matches Select[int]."""
    accepts_select_int(
        select({
            "DEFAULT": 0,
            "config//os:linux": 42,
        })
    )

def accepts_select_list_str(x: Select[list[str]]) -> bool:
    """Expects Select[list[str]]."""
    return isinstance(x, Select)

def call_with_matching_list_branches():
    """All branches are list[str], which matches Select[list[str]]."""
    accepts_select_list_str(
        select({
            "DEFAULT": ["a", "b"],
            "config//os:linux": ["c"],
        })
    )
