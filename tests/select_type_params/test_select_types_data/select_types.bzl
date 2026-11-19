# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def returns_bare_select() -> Select:
    """Function returning a bare Select type."""
    return select({
        "DEFAULT": "value",
    })

def accepts_bare_select(x: Select) -> bool:
    """Function accepting a bare Select parameter."""
    return isinstance(x, Select)

def select_in_union(x: str | Select) -> bool:
    """Function with Select in a union type annotation."""
    return isinstance(x, Select)

def select_or_none(x: Select | None = None) -> bool:
    """Function with optional Select parameter."""
    return x != None

def identity_select(x: Select) -> Select:
    """Function that takes and returns a Select."""
    return x

def returns_parameterized_select() -> Select[str]:
    """Function returning a parameterized Select[str] type."""
    return select({
        "DEFAULT": "value",
    })

def accepts_parameterized_select(x: Select[str]) -> bool:
    """Function accepting a parameterized Select[str] parameter."""
    return isinstance(x, Select)

def parameterized_select_in_union(x: str | Select[str]) -> bool:
    """Function with Select[str] in a union type annotation."""
    return isinstance(x, Select)

def parameterized_select_or_none(x: Select[int] | None = None) -> bool:
    """Function with optional parameterized Select parameter."""
    return x != None

def identity_parameterized_select(x: Select[str]) -> Select[str]:
    """Function that takes and returns a parameterized Select[str]."""
    return x
