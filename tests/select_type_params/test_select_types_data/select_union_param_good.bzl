# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test that Select[str] | list[str] union accepts both parameterized types.

def accepts_select_or_list(x: Select[str] | list[str]) -> bool:
    """Accepts either a select expression or a list of strings."""
    return isinstance(x, Select)

def good_with_select():
    accepts_select_or_list(select({"DEFAULT": "hello"}))

def good_with_list():
    accepts_select_or_list(["hello", "world"])
