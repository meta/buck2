# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Open source tests have no network policy to declare: every value leaves a
# test's `network_access` unset.

def _unset(*_args, **_kwargs):
    return None

network_access_utils = struct(
    all = _unset,
    loopback = _unset,
    none = _unset,
    private = _unset,
    set_package_default = _unset,
    strict = _unset,
)
