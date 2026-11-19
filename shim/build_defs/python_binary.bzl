# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def python_binary(name, srcs = [], main_src = None, deps = [], base_module = None, visibility = ["PUBLIC"], **kwargs):
    native.python_library(
        name = name + "-library",
        srcs = srcs,
        base_module = base_module,
        visibility = [],
    )
    if main_src != None:
        if "main" not in kwargs:
            kwargs["main"] = main_src

    # @lint-ignore BUCKLINT: avoid "Direct usage of native rules is not allowed."
    native.python_binary(name = name, deps = deps + [":" + name + "-library"], base_module = base_module, visibility = visibility, **kwargs)
