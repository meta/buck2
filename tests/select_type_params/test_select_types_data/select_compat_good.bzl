# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test cases for backward compatibility with bare (unparameterized) Select type.
# Bare Select should accept any select() expression regardless of branch value types.
# All functions here should pass type checking.

def accepts_bare_select(x: Select) -> bool:
    """Bare Select accepts any select expression."""
    return isinstance(x, Select)

def call_bare_with_str_select():
    """Bare Select accepts select() with str branches."""
    accepts_bare_select(select({"DEFAULT": "value"}))

def call_bare_with_int_select():
    """Bare Select accepts select() with int branches."""
    accepts_bare_select(select({"DEFAULT": 42}))

def call_bare_with_bool_select():
    """Bare Select accepts select() with bool branches."""
    accepts_bare_select(select({"DEFAULT": True}))

def call_bare_with_list_select():
    """Bare Select accepts select() with list branches."""
    accepts_bare_select(select({"DEFAULT": ["a", "b"]}))

def call_bare_with_dict_select():
    """Bare Select accepts select() with dict branches."""
    accepts_bare_select(select({"DEFAULT": {"key": 1}}))

def call_bare_with_none_select():
    """Bare Select accepts select() with None branches."""
    accepts_bare_select(select({"DEFAULT": None}))

def bare_select_in_union(x: Select | str) -> bool:
    """Bare Select in union."""
    return isinstance(x, Select)

def call_bare_union_with_str_select():
    """Bare Select | str accepts select()."""
    bare_select_in_union(select({"DEFAULT": "value"}))

def call_bare_union_with_plain_str():
    """Bare Select | str accepts plain str."""
    bare_select_in_union("plain")

def bare_optional(x: Select | None = None) -> bool:
    """Optional bare Select."""
    return x != None

def call_bare_optional_with_select():
    """Optional bare Select accepts select()."""
    bare_optional(select({"DEFAULT": 42}))

def call_bare_optional_with_none():
    """Optional bare Select accepts None."""
    bare_optional(None)

def returns_bare_select() -> Select:
    """Function returning bare Select."""
    return select({"DEFAULT": "value"})

def identity_bare_select(x: Select) -> Select:
    """Function that takes and returns bare Select."""
    return x
