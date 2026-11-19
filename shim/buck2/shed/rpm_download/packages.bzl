# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# Downloading an RPM needs the host's dnf, which an open source build cannot
# count on: the target is an empty directory, so the binaries whose resources
# name it still build.
def download_rpm(name, rpm_name, **_kwargs):
    _unused = rpm_name  # @unused
    native.filegroup(
        name = name,
        srcs = [],
        visibility = ["PUBLIC"],
    )
