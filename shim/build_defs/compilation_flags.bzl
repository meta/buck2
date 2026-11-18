# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# The warning flags `PACKAGE` files set with this apply to Meta's C++
# toolchains only.

def _automatically_managed_warning_flags(*_args, **_kwargs):
    pass

compilation_flags = struct(
    automatically_managed_warning_flags = _automatically_managed_warning_flags,
)
