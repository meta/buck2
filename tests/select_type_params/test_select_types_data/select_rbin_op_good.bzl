# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Test cases for reverse binary operations (rbin_op) with Select[T].
# Verifies that `non_select + Select[T]` type-checks successfully.
# This exercises the TyBasic::Custom arm in expr_bin_op_ty_basic_rhs.

def list_plus_select() -> Select[list[str]]:
    """list + Select[list[str]] should succeed via rbin_op dispatch."""
    return [] + select({"DEFAULT": ["a", "b"]})

def list_plus_select_in_loop() -> list[typing.Never] | Select[list[str]]:
    """Accumulating selects in a loop (the tags_helper.bzl pattern)."""
    selects = []
    tags = ["tag1", "tag2"]
    for tag in tags:
        selects += select({
            "DEFAULT": [],
            "config//:" + tag: [tag],
        })
    return selects

def str_plus_select() -> Select[str]:
    """str + Select[str] should succeed via rbin_op dispatch."""
    return "prefix_" + select({"DEFAULT": "value"})
