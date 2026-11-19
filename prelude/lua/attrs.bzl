# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _srcs_arg():
    return {
        "srcs": attrs.named_set(
            attrs.source(),
            sorted = True,
            default = [],
            doc = """
    The set of `.lua` files included in this library.
""",
        ),
    }

def _base_module_arg():
    return {
        "base_module": attrs.option(
            attrs.string(),
            default = None,
            doc = """
    The package for which the given specified sources and resources should reside in their final
     location in the top-level binary. If unset, the project relative directory that houses the
     BUCK file is used.
""",
        ),
    }

lua_common = struct(
    srcs_arg = _srcs_arg,
    base_module_arg = _base_module_arg,
)
