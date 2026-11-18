# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# The build modes that many of Meta's directories share. Build modes do
# nothing in open source builds (see `build_defs/build_mode.bzl`).

def get_empty_modes():
    return None

def restore_all_modes():
    return None

def all_modes_use_no_lsan():
    return None

def all_modes_keep_gpu_sections_all_modes_use_lld():
    return None

def all_modes_strict():
    return None
