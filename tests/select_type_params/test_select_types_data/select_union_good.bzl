# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test cases that validate Select[T] | T union semantics.
# All functions here should pass type checking.

def accepts_select_or_str(x: Select[str] | str) -> bool:
    """Union type accepts both a select expression and a plain string."""
    return isinstance(x, Select)

def call_with_select():
    """Passing a select() expression to a Select[str] | str parameter is valid."""
    accepts_select_or_str(select({"DEFAULT": "value"}))

def call_with_plain_str():
    """Passing a plain string to a Select[str] | str parameter is valid."""
    accepts_select_or_str("plain value")

def accepts_select_only(x: Select[str]) -> bool:
    """Select[str] only accepts select expressions."""
    return isinstance(x, Select)

def call_select_only_with_select():
    """Passing a select() expression to a Select[str] parameter is valid."""
    accepts_select_only(select({"DEFAULT": "value"}))

def accepts_bare_select_or_str(x: Select | str) -> bool:
    """Bare Select union with str accepts both forms."""
    return isinstance(x, Select)

def call_bare_union_with_select():
    """Passing a select() expression to a Select | str parameter is valid."""
    accepts_bare_select_or_str(select({"DEFAULT": "value"}))

def call_bare_union_with_str():
    """Passing a plain string to a Select | str parameter is valid."""
    accepts_bare_select_or_str("plain value")

def select_or_int(x: Select[int] | int) -> bool:
    """Union with non-string type also works."""
    return isinstance(x, Select)

def call_select_or_int_with_select():
    """Passing a select() to a Select[int] | int parameter is valid."""
    select_or_int(select({"DEFAULT": 42}))

def call_select_or_int_with_int():
    """Passing a plain int to a Select[int] | int parameter is valid."""
    select_or_int(42)
