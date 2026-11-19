# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test cases demonstrating that Select[str] != Select[str] | str.
# Select[str] alone does NOT accept plain strings; only the union form does.
# All functions here should pass type checking.

def strict_select_only(x: Select[str]) -> bool:
    """Select[str] without union only accepts select expressions."""
    return isinstance(x, Select)

def lenient_select_or_str(x: Select[str] | str) -> bool:
    """Select[str] | str union accepts both select expressions AND plain strings."""
    return isinstance(x, Select)

def call_strict_with_select():
    """Select expression is valid for Select[str]."""
    strict_select_only(select({"DEFAULT": "value"}))

def call_lenient_with_select():
    """Select expression is valid for Select[str] | str."""
    lenient_select_or_str(select({"DEFAULT": "value"}))

def call_lenient_with_str():
    """Plain string is valid for Select[str] | str."""
    lenient_select_or_str("plain value")

def strict_int_only(x: Select[int]) -> bool:
    """Select[int] without union only accepts select expressions."""
    return isinstance(x, Select)

def lenient_int_or_int(x: Select[int] | int) -> bool:
    """Select[int] | int union accepts both forms."""
    return isinstance(x, Select)

def call_strict_int_with_select():
    """Select expression is valid for Select[int]."""
    strict_int_only(select({"DEFAULT": 42}))

def call_lenient_int_with_select():
    """Select expression is valid for Select[int] | int."""
    lenient_int_or_int(select({"DEFAULT": 42}))

def call_lenient_int_with_int():
    """Plain int is valid for Select[int] | int."""
    lenient_int_or_int(42)
