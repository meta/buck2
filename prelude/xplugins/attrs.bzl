# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(":types.bzl", "XPluginsManifestInfo")

_DEBUG_ARTIFACTS_ATTR = {"xplugins_debug_artifacts": attrs.option(attrs.dep(providers = [XPluginsManifestInfo]), default = None)}

xplugins_common = struct(
    debug_artifacts_arg = _DEBUG_ARTIFACTS_ATTR,
)
