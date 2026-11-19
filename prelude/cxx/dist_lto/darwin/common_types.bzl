# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//linking:link_info.bzl",
    "SharedLibLinkable",  # @unused Used as a type
)

EagerBitcodeLinkData = record(
    name = str,
    input_object_file = Artifact,  # This may be a native object file or a bitcode file, we can't tell at this point
    output_index_shard_file = Artifact,
    plan = Artifact,
    output_first_codegen_round_native_object_file = field([Artifact, None]),
    output_precodegen_ir = field([Artifact, None]),
    output_final_native_object_file = Artifact,
    extra_outputs = field(dict[str, Artifact]),
)

LazyBitcodeLinkData = record(
    name = str,
    input_object_file = Artifact,  # This may be a native object file or a bitcode file, we can't tell at this point
    output_index_shard_file = Artifact,
    plan = Artifact,
    output_final_native_object_file = Artifact,
    output_first_codegen_round_native_object_file = field([Artifact, None]),
    output_precodegen_ir = field([Artifact, None]),
    archive_start = bool,
    archive_end = bool,
    extra_outputs = field(dict[str, Artifact]),
)

DynamicLibraryLinkData = record(
    linkable = SharedLibLinkable,
)

LinkDataType = enum(
    "eager_bitcode",
    "lazy_bitcode",
    "dynamic_library",
)

DThinLTOLinkData = record(
    data_type = LinkDataType,
    link_data = field([LazyBitcodeLinkData, EagerBitcodeLinkData, DynamicLibraryLinkData]),
)
