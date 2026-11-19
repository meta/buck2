# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test cases for Select[bool] parameterization.
# All functions here should pass type checking.

def accepts_select_bool(x: Select[bool]) -> bool:
    """Select parameterized with bool."""
    return isinstance(x, Select)

def call_select_bool_true():
    """Passing a select() resolving to True is valid for Select[bool]."""
    accepts_select_bool(select({"DEFAULT": True}))

def call_select_bool_false():
    """Passing a select() resolving to False is valid for Select[bool]."""
    accepts_select_bool(select({"DEFAULT": False}))

def accepts_select_bool_or_bool(x: Select[bool] | bool) -> bool:
    """Union of Select[bool] with bool accepts both forms."""
    return isinstance(x, Select)

def call_union_with_select_bool():
    """Passing a select() to Select[bool] | bool is valid."""
    accepts_select_bool_or_bool(select({"DEFAULT": True}))

def call_union_with_plain_bool():
    """Passing a plain bool to Select[bool] | bool is valid."""
    accepts_select_bool_or_bool(False)

def returns_select_bool() -> Select[bool]:
    """Function returning Select[bool]."""
    return select({"DEFAULT": True})

def identity_select_bool(x: Select[bool]) -> Select[bool]:
    """Function that takes and returns Select[bool]."""
    return x
