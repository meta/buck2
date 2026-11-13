# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Open source has no remote execution for tests: no labels ask for it.

def _labels_have_re_signal(_labels):
    return False

re_test_utils = struct(
    labels_have_re_signal = _labels_have_re_signal,
)
