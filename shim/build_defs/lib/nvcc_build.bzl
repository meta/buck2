# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Open source has no CUDA toolchain for this to configure.

def _set_nvcc_host_compiler(*_args, **_kwargs):
    pass

nvcc_build = struct(
    set_nvcc_host_compiler = _set_nvcc_host_compiler,
)
