# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

XCODE_DATA_SUB_TARGET = "xcode-data"
_XCODE_DATA_FILE_NAME = "xcode_data.json"

XcodeDataInfo = provider(
    fields = {
        "data": provider_field(typing.Any, default = None),  # {str: _a}
    }
)

XcodeDataInfoKeys = struct(
    APP_EXTENSION_DEPENDENCIES = "app_extension_dependencies",
    ARCH = "arch",
    ARGSFILES_BY_EXT = "argsfiles_by_ext",
    BUNDLE_TYPE = "bundle_type",
    CONTAINS_SWIFT_SOURCES = "contains_swift_sources",
    DEFAULT_TARGET_PLATFORM = "default_target_platform",
    DEPLOYMENT_VERSION = "deployment_version",
    EXPORTED_HEADERS = "exported_headers",
    EXTRA_XCODE_FILES = "extra_xcode_files",
    HEADERS = "headers",
    INFO_PLIST = "info_plist",
    OUTPUT = "output",
    PROCESSED_INFO_PLIST = "processed_info_plist",
    INFO_PLIST_RELATIVE_PATH = "info_plist_relative_path",
    PRODUCT_NAME = "product_name",
    RULE_TYPE = "rule_type",
    SDK = "sdk",
    SRCS = "srcs",
    SWIFT_VERSION = "swift_version",
    TARGET = "target",
    TEST_HOST_APP_BINARY = "test_host_app_binary",
    TEST_HOST_APP_BUNDLE = "test_host_app_bundle",
    TEST_HOST_APP_TARGET = "test_host_app_target",
    TEST_TARGET = "test_target",
    TEST_TYPE = "test_type",
    XCTOOLCHAIN_BUNDLE_ID_TARGET = "xctoolchain_bundle_id_target",
    XCTOOLCHAIN_BUNDLE_ID = "xctoolchain_bundle_id",
    XCTOOLCHAIN_BUNDLE_TARGET = "xctoolchain_bundle_target",
    TEST_TARGET_REFS_IN_CURRENT_TARGET = "test_target_refs_in_current_target",
)

def generate_xcode_data(
    ctx: AnalysisContext, rule_type: str, output: Artifact | None, populate_rule_specific_attributes_func: [typing.Callable, None] = None, **kwargs
) -> (list[DefaultInfo], XcodeDataInfo):
    data = {
        XcodeDataInfoKeys.RULE_TYPE: rule_type,
        XcodeDataInfoKeys.TARGET: ctx.label,
    }
    if output:
        data[XcodeDataInfoKeys.OUTPUT] = output

    data[XcodeDataInfoKeys.EXTRA_XCODE_FILES] = []
    if hasattr(ctx.attrs, "extra_xcode_files"):
        data[XcodeDataInfoKeys.EXTRA_XCODE_FILES] = ctx.attrs.extra_xcode_files

    if populate_rule_specific_attributes_func:
        data.update(populate_rule_specific_attributes_func(ctx, **kwargs))

    json_file = ctx.actions.write_json(_XCODE_DATA_FILE_NAME, data, has_content_based_path = False)
    return [DefaultInfo(default_output = json_file)], XcodeDataInfo(data = data)
