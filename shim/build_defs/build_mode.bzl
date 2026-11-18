# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Internally, a `PACKAGE` file sets a build mode to adjust compiler, linker and
# sanitizer settings for the directories below it, and Meta's macros apply it.
# Open source builds have no such adjustment, so setting one does nothing.

def set_build_mode(_build_mode):
    pass

def set_fbcode_build_mode(_build_mode):
    pass

def extend_fbcode_build_mode(**_kwargs):
    pass

def set_unified_build_mode(**_kwargs):
    pass
