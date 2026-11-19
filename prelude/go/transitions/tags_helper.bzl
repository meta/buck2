# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//utils:buckconfig.bzl", "read_list")

allowed_build_tags = read_list("go", "allowed_build_tags", default = [], root_cell = True)

def tag_to_constraint_value():
    return {tag: "prelude//go/tags/constraints:{}[set]".format(tag) for tag in allowed_build_tags}

def selects_for_tags():
    selects = []
    for tag in allowed_build_tags:
        selects += select({
            "prelude//go/tags/constraints:{}[set]".format(tag): [tag],
            "prelude//go/tags/constraints:{}[unset]".format(tag): [],
        })

    return selects
