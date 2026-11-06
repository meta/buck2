# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# The package defaults `PACKAGE` files set with these (rustc and Miri flags,
# the clippy configuration) are read by Meta's Rust macros.  The shim's Rust
# macros take their flags from `rust_oss.bzl` instead.

def _noop(*_args, **_kwargs):
    pass

rust_package_local = struct(
    set_clippy_configuration = _noop,
    set_miri = _noop,
    set_miri_flags = _noop,
    set_miri_interpreted = _noop,
    set_rustc_flags = _noop,
    set_unittests_miri = _noop,
)
