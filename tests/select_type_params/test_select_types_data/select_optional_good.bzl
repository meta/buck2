# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test cases for optional types within Select type parameters.
# Select[str | None] represents a select expression that may resolve to str or None.
# All functions here should pass type checking.

def accepts_select_str_or_none(x: Select[str | None]) -> bool:
    """Select parameterized with str | None."""
    return isinstance(x, Select)

def call_select_str_or_none_with_str():
    """Passing a select() resolving to str is valid for Select[str | None]."""
    accepts_select_str_or_none(select({"DEFAULT": "value"}))

def call_select_str_or_none_with_none():
    """Passing a select() resolving to None is valid for Select[str | None]."""
    accepts_select_str_or_none(select({"DEFAULT": None}))

def accepts_select_int_or_none(x: Select[int | None]) -> bool:
    """Select parameterized with int | None."""
    return isinstance(x, Select)

def call_select_int_or_none_with_int():
    """Passing a select() resolving to int is valid for Select[int | None]."""
    accepts_select_int_or_none(select({"DEFAULT": 42}))

def call_select_int_or_none_with_none():
    """Passing a select() resolving to None is valid for Select[int | None]."""
    accepts_select_int_or_none(select({"DEFAULT": None}))

def select_optional_in_union(x: Select[str | None] | str | None) -> bool:
    """Union of Select[str | None] with str | None accepts all forms."""
    return isinstance(x, Select)

def call_optional_union_with_select():
    """Passing a select() to Select[str | None] | str | None is valid."""
    select_optional_in_union(select({"DEFAULT": "value"}))

def call_optional_union_with_str():
    """Passing a plain str to Select[str | None] | str | None is valid."""
    select_optional_in_union("plain value")

def call_optional_union_with_none():
    """Passing None to Select[str | None] | str | None is valid."""
    select_optional_in_union(None)

def returns_select_str_or_none() -> Select[str | None]:
    """Function returning Select[str | None]."""
    return select({"DEFAULT": "value"})

def identity_select_optional(x: Select[str | None]) -> Select[str | None]:
    """Function that takes and returns Select[str | None]."""
    return x
