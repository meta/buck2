# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

MockingbirdLibraryInfoTSet = transitive_set()

MockingbirdTargetType = enum("library", "test")

MockingbirdLibraryInfo = provider(
    fields = {
        # The Swift module name.
        "name": provider_field(str),
        # Contains a tset with this module's MockingbirdLibraryRecord as the value
        # and all of its dependencies' MockingbirdLibraryRecord in the children.
        "tset": provider_field(MockingbirdLibraryInfoTSet),
    },
)

MockingbirdLibraryRecord = record(
    # The Swift module names of this module's dependencies.
    dep_names = field(list[str]),
    # The Swift module names of this module's exported dependencies.
    exported_dep_names = field(list[str]),
    # The Swift module name.
    name = str,
    # Swift sources in this target.
    srcs = field(list[Artifact]),
    # Whether this is a library or a test.
    type = field(MockingbirdTargetType),
    # Symlinked directory containing the source files.
    src_dir = field(Artifact),
)

MockingbirdSourcesInfo = provider(
    fields = {
        # Source files containing the auto generated mocks produced by mockingbird-cli.
        "srcs": provider_field(list[Artifact]),
    },
)

MockingbirdToolchainInfo = provider(
    fields = {
        "bin": cmd_args,
        "support": Artifact,
    }
)
