# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test cases for nested type parameters within Select.
# All functions here should pass type checking.

def accepts_select_list_str(x: Select[list[str]]) -> bool:
    """Select parameterized with list[str]."""
    return isinstance(x, Select)

def call_select_list_str():
    """Passing a select() expression to Select[list[str]] is valid."""
    accepts_select_list_str(select({"DEFAULT": ["a", "b"]}))

def accepts_select_dict_str_int(x: Select[dict[str, int]]) -> bool:
    """Select parameterized with dict[str, int]."""
    return isinstance(x, Select)

def call_select_dict_str_int():
    """Passing a select() expression to Select[dict[str, int]] is valid."""
    accepts_select_dict_str_int(select({"DEFAULT": {"key": 1}}))

def accepts_select_list_dict(x: Select[list[dict[str, bool]]]) -> bool:
    """Select parameterized with list[dict[str, bool]] (multiple nesting levels)."""
    return isinstance(x, Select)

def call_select_list_dict():
    """Passing a select() expression to Select[list[dict[str, bool]]] is valid."""
    accepts_select_list_dict(select({"DEFAULT": [{"flag": True}]}))

def select_list_str_union(x: Select[list[str]] | list[str]) -> bool:
    """Union of Select[list[str]] with list[str]."""
    return isinstance(x, Select)

def call_union_with_select():
    """Passing a select() to Select[list[str]] | list[str] is valid."""
    select_list_str_union(select({"DEFAULT": ["a"]}))

def call_union_with_plain():
    """Passing a plain list[str] to Select[list[str]] | list[str] is valid."""
    select_list_str_union(["a", "b"])
