# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolsInfo")

def apple_tools_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        AppleToolsInfo(
            assemble_bundle = ctx.attrs.assemble_bundle[RunInfo],
            provisioning_manifest = ctx.attrs.provisioning_manifest[RunInfo],
            resolve_signing_context = ctx.attrs.resolve_signing_context[RunInfo],
            signing_info = ctx.attrs.signing_info[RunInfo],
            split_arch_combine_dsym_bundles_tool = ctx.attrs.split_arch_combine_dsym_bundles_tool[RunInfo],
            dry_codesign_tool = ctx.attrs.dry_codesign_tool[RunInfo],
            adhoc_codesign_tool = ctx.attrs.adhoc_codesign_tool[RunInfo],
            codesign_manifest_tree_postprocessor = ctx.attrs.codesign_manifest_tree_postprocessor[RunInfo],
            dedupe_swift_module_map = ctx.attrs.dedupe_swift_module_map[RunInfo],
            signing_context_tree_postprocessor = ctx.attrs.signing_context_tree_postprocessor[RunInfo],
            info_plist_processor = ctx.attrs.info_plist_processor[RunInfo],
            ipa_package_maker = ctx.attrs.ipa_package_maker[RunInfo],
            make_modulemap = ctx.attrs.make_modulemap[RunInfo],
            make_vfsoverlay = ctx.attrs.make_vfsoverlay[RunInfo],
            selective_debugging_scrubber = ctx.attrs.selective_debugging_scrubber[RunInfo],
            selective_debugging_dsym_allowlist_tool = ctx.attrs.selective_debugging_dsym_allowlist_tool[RunInfo],
            xcframework_maker = ctx.attrs.xcframework_maker[RunInfo],
            framework_sanitizer = ctx.attrs.framework_sanitizer[RunInfo],
            static_archive_linker = ctx.attrs.static_archive_linker[RunInfo],
            spm_packager = ctx.attrs.spm_packager[RunInfo],
            bundle_telemetry_logger = ctx.attrs.bundle_telemetry_logger[RunInfo] if ctx.attrs.bundle_telemetry_logger else None,
            swiftmodule_change_analysis = ctx.attrs.swiftmodule_change_analysis[RunInfo] if ctx.attrs.swiftmodule_change_analysis else None,
            make_swift_dynamic_thunk = ctx.attrs.make_swift_dynamic_thunk[RunInfo] if ctx.attrs.make_swift_dynamic_thunk else None,
            make_swift_dynamic_thunk_command_database = ctx.attrs.make_swift_dynamic_thunk_command_database[RunInfo]
            if ctx.attrs.make_swift_dynamic_thunk_command_database
            else None,
        ),
    ]
