# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//ide_integrations/xcode:data.bzl",
    "XCODE_DATA_SUB_TARGET",
    "XcodeDataInfoKeys",
    "generate_xcode_data",
)
load("@prelude//user:rule_spec.bzl", "RuleRegistrationSpec")

# A rule that exposes files to Xcode (in project generation).

def _xcode_files_impl(ctx: AnalysisContext) -> list[Provider]:
    xcode_data_default_info, xcode_data_info = generate_xcode_data(ctx, "xcode_files", None, _xcode_populate_attributes)

    return [
        DefaultInfo(
            default_output = None,
            sub_targets = {
                XCODE_DATA_SUB_TARGET: xcode_data_default_info,
            },
        ),
        xcode_data_info,
    ]

def _xcode_populate_attributes(ctx) -> dict[str, typing.Any]:
    data = {XcodeDataInfoKeys.EXTRA_XCODE_FILES: ctx.attrs.files}
    return data

registration_spec = RuleRegistrationSpec(
    name = "xcode_files",
    impl = _xcode_files_impl,
    attrs = {
        "files": attrs.list(
            attrs.option(attrs.source(allow_directory = True)), default = [], doc = """List of paths to the file or folders that should be included in Xcode."""
        ),
        "labels": attrs.list(attrs.string(), default = []),
    },
)
