# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test cases that should FAIL type checking.
# When select() branch values do not match the expected type parameter,
# the type checker should report errors.

def accepts_select_str(x: Select[str]) -> bool:
    """Expects Select[str]."""
    return isinstance(x, Select)

def bad_call_int_branch_to_select_str():
    """ERROR: select() with int branches does not match Select[str]."""
    accepts_select_str(select({"DEFAULT": 42}))

def bad_call_bool_branch_to_select_str():
    """ERROR: select() with bool branches does not match Select[str]."""
    accepts_select_str(select({"DEFAULT": True}))
