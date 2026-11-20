# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl(_ctx):
    return [DefaultInfo()]

record = rule(
    impl = _impl,
    attrs = {
        "cells": attrs.dict(attrs.string(), attrs.string()),
    },
)

def _cells():
    return {
        "get_cell_name": get_cell_name(),
        "read_config": read_config("fixture", "cell"),
    }

TOP_LEVEL = _cells()

def in_function():
    return _cells()

# One line for each time this file is evaluated.
print("evaluating defs.bzl")  # buildifier: disable=print
