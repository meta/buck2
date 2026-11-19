# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(":apple_bundle_types.bzl", "AppleBundleInfo", "ApplePackageExtension")

ApplePackageInfo = provider(
    fields = {
        "bundle_info": provider_field(AppleBundleInfo),
        "dsyms": provider_field(list[Artifact]),
        "extension": provider_field(ApplePackageExtension),
        "info_plist": provider_field(Artifact),
        "linker_maps": provider_field(list[Artifact]),
        "name": provider_field(str),
        "package": provider_field(Artifact),
        "unstripped_binaries": provider_field(list[Artifact]),
    },
)
