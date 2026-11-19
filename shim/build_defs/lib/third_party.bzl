# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Internally, `versions` turns third-party version pins into cfg modifiers on
# Meta's platform constraints.  Open source has neither, so no modifiers.

def _versions(_versions, **_kwargs):
    return []

third_party = struct(
    versions = _versions,
)
