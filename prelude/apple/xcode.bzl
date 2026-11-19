# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//apple:apple_sdk.bzl", "get_apple_sdk_name")
load("@prelude//apple:apple_target_sdk_version.bzl", "get_min_deployment_version_for_node")
load("@prelude//apple:apple_utility.bzl", "get_apple_architecture", "has_apple_toolchain")
load(
    "@prelude//cxx:argsfiles.bzl",
    "CompileArgsfile",  # @unused Used as a type
)
load(
    "@prelude//cxx:cxx_sources.bzl",
    "CxxSrcWithFlags",  # @unused Used as a type
)
load("@prelude//cxx:xcode.bzl", "cxx_populate_xcode_attributes")
load("@prelude//ide_integrations/xcode:data.bzl", "XcodeDataInfoKeys")
load("@prelude//utils:expect.bzl", "expect")

def apple_populate_xcode_attributes(
    ctx, srcs: list[CxxSrcWithFlags], argsfiles: dict[str, CompileArgsfile], product_name: str, contains_swift_sources: bool = False
) -> dict[str, typing.Any]:
    data = cxx_populate_xcode_attributes(ctx = ctx, srcs = srcs, argsfiles = argsfiles, product_name = product_name)

    data[XcodeDataInfoKeys.CONTAINS_SWIFT_SOURCES] = contains_swift_sources

    if has_apple_toolchain(ctx):
        data[XcodeDataInfoKeys.ARCH] = get_apple_architecture(ctx)
        data[XcodeDataInfoKeys.SDK] = get_apple_sdk_name(ctx)
        data[XcodeDataInfoKeys.DEPLOYMENT_VERSION] = get_min_deployment_version_for_node(ctx)

    if hasattr(ctx.attrs, "swift_version"):
        swift_version = ctx.attrs.swift_version
        if swift_version != None:
            data[XcodeDataInfoKeys.SWIFT_VERSION] = swift_version

    apple_xcode_data_add_xctoolchain(ctx, data)
    return data

def apple_xcode_data_add_xctoolchain(ctx: AnalysisContext, data: dict[str, typing.Any]):
    _add_label_for_attr(ctx, "_apple_xctoolchain_bundle_id", "xctoolchain_bundle_id_target", data)
    _add_output_for_attr(ctx, "_apple_xctoolchain_bundle_id", "xctoolchain_bundle_id", data)
    _add_label_for_attr(ctx, "_apple_xctoolchain", "xctoolchain_bundle_target", data)

def _add_label_for_attr(ctx: AnalysisContext, attr_name: str, field_name: str, data: dict[str, typing.Any]):
    xctoolchain_dep = _get_attribute_with_output(ctx, attr_name)
    if xctoolchain_dep:
        data[field_name] = xctoolchain_dep.label

def _add_output_for_attr(ctx: AnalysisContext, attr_name: str, field_name: str, data: dict[str, typing.Any]):
    xctoolchain_dep = _get_attribute_with_output(ctx, attr_name)
    if xctoolchain_dep:
        default_info = xctoolchain_dep[DefaultInfo]
        expect(len(default_info.default_outputs) == 1, "Expected only one output, got {}", len(default_info.default_outputs))
        data[field_name] = default_info.default_outputs[0]

def _get_attribute_with_output(ctx: AnalysisContext, attr_name: str) -> [Dependency, None]:
    if hasattr(ctx.attrs, attr_name):
        dep = getattr(ctx.attrs, attr_name)
        default_info = dep[DefaultInfo]
        if len(default_info.default_outputs) > 0:
            # When there's no xctoolchain, there will be an empty `DefaultInfo`.
            # So, an empty `DefaultInfo` basically signifies that there's no xctoolchain.
            return dep
    return None
