# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl(_ctx):
    return [DefaultInfo()]

toolchain = rule(impl = _impl, attrs = {}, is_toolchain_rule = True)

uses_toolchain = rule(
    impl = _impl,
    attrs = {"toolchain": attrs.toolchain_dep(default = "root//:tc")},
)
