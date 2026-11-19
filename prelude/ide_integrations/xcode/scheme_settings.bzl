# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

XCODE_SCHEME_SETTINGS_ATTR_NAME = "xcode_scheme_settings"
XCODE_SCHEME_SETTINGS_ATTR_TYPE = attrs.option(
    attrs.source(), default = None, doc = "Optional settings to set on schemes when this target is represented in Xcode."
)
