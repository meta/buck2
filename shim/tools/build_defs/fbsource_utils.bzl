# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def is_arvr_mode():
    return False

def is_fbcode():
    return True

def is_xplat():
    return False

def is_fbcode_compatible():
    return False

def is_fbcode_mode_mac():
    return False

def is_fbcode_mode_win():
    return False
