# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

GoBootstrapDistrInfo = provider(
    # @unsorted-dict-items
    fields = {
        "bin_go": provider_field(RunInfo),
        "go_root": provider_field(Artifact | None),
    },
)

GoBootstrapToolchainInfo = provider(
    fields = {
        "env_go_arch": provider_field(str),
        "env_go_os": provider_field(str),
        "env_go_root": provider_field(Artifact | None, default = None),
        "go": provider_field(RunInfo),
        "go_wrapper": provider_field(RunInfo),
    },
)
