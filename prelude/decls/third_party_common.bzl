# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _create_third_party_build_root_attr():
    return (
        "_create_third_party_build_root",
        attrs.default_only(
            attrs.exec_dep(
                default = select({
                    "DEFAULT": "prelude//third-party/tools:create_build-full",
                    # Neutral linux-fat configs take the construction build root;
                    # the key is a platform-only constraint value that never
                    # appears in OSS configurations.
                    # @oss-disable[end= ]: "fbcode//buck2/platform/execution/linux_fat:linux-fat": "prelude//third-party/tools:create_build-construction",
                    # @oss-disable[end= ]: "prelude//third-party/tools:purpose[construction]": "prelude//third-party/tools:create_build-construction",
                }),
                exec_group = DefaultExecGroup,
            )
        ),
    )

def _create_third_party_build_root_attrs():
    key, val = _create_third_party_build_root_attr()
    return {key: val}

third_party_common = struct(
    create_third_party_build_root_attr = _create_third_party_build_root_attr,
    create_third_party_build_root_attrs = _create_third_party_build_root_attrs,
)
