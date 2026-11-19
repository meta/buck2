# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Ways a library can request to be linked (e.g. usually specific via a rule
# param like `preferred_linkage`). The actual link style used for a library is
# usually determined by a combination of this and the link style being exported
# via a provider.
Linkage = enum(
    "any",
    "static",
    "shared",
)

# How much of the link step may be redone when inputs change.
LinkPreference = enum("default", "full", "incremental")
