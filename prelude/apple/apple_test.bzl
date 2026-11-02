# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(
    "@prelude//:artifact_tset.bzl",
    "project_artifacts",
)
load("@prelude//apple:apple_library.bzl", "AppleLibraryAdditionalParams", "apple_library_rule_constructor_params_and_swift_providers")
load("@prelude//apple:apple_test_device_types.bzl", "AppleTestDeviceType", "get_default_test_device", "tpx_label_for_test_device_type")
load("@prelude//apple:apple_test_frameworks_utility.bzl", "get_test_frameworks_bundle_parts")
load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")
# @oss-disable[end= ]: load("@prelude//apple/meta_only:apple_test_local_execution.bzl", "local_test_execution_is_available")
# @oss-disable[end= ]: load("@prelude//apple/meta_only:apple_test_re_capabilities.bzl", "apple_test_re_capabilities")
# @oss-disable[end= ]: load("@prelude//apple/meta_only:apple_test_re_use_case.bzl", "apple_test_re_use_case")
load("@prelude//apple/swift:swift_compilation.bzl", "get_swift_anonymous_targets")
load("@prelude//apple/swift:swift_helpers.bzl", "uses_explicit_modules")
load(
    "@prelude//cxx:argsfiles.bzl",
    "CompileArgsfile",  # @unused Used as a type
)
load("@prelude//cxx:cxx_library.bzl", "cxx_library_parameterized")
load(
    "@prelude//cxx:cxx_sources.bzl",
    "CxxSrcWithFlags",  # @unused Used as a type
)
load("@prelude//cxx:cxx_types.bzl", "CxxRuleProviderParams", "CxxRuleSubTargetParams", "xcode_data_enabled")
load(
    "@prelude//cxx:linker.bzl",
    "SharedLibraryFlagOverrides",
)
load("@prelude//ide_integrations/xcode:data.bzl", "XcodeDataInfoKeys")
load("@prelude//target_stats:target_stats_types.bzl", "TargetStatsInfo")
load(
    "@prelude//utils:dicts.bzl",
    "flatten_x",
)
load("@prelude//utils:expect.bzl", "expect")
load(":apple_bundle.bzl", "AppleBundlePartListConstructorParams", "get_apple_bundle_part_list")
load(":apple_bundle_destination.bzl", "AppleBundleDestination", "bundle_relative_path_for_destination")
load(":apple_bundle_part.bzl", "AppleBundlePart", "assemble_bundle", "bundle_output", "get_bundle_dir_name")
load(":apple_bundle_types.bzl", "AppleBundleInfo", "AppleBundleResourceInfo")
load(":apple_bundle_utility.bzl", "get_product_name")
load(
    ":apple_dsym.bzl",
    "DSYM_SUBTARGET",
    "DWARF_AND_DSYM_SUBTARGET",
    "EXTENDED_DSYM_INFO_SUBTARGET",
    "get_apple_dsym",
    "get_apple_dsym_info_json",
    "get_deps_debuggable_infos",
)
load(":apple_entitlements.bzl", "entitlements_link_flags")
load(":apple_rpaths.bzl", "get_rpath_flags_for_tests")
load(":apple_sdk.bzl", "get_apple_sdk_name")
load(":apple_sdk_metadata.bzl", "WatchSimulatorSdkMetadata")
load(":debug.bzl", "AppleDebuggableInfo")
load(":xcode.bzl", "apple_populate_xcode_attributes")
load(":xctest_swift_support.bzl", "XCTestSwiftSupportInfo")

_XCTOOLCHAIN_SUB_TARGET = "xctoolchain"

def apple_test_impl(ctx: AnalysisContext) -> [list[Provider], Promise]:
    def get_apple_test_providers(deps_providers) -> list[Provider]:
        xctest_bundle = bundle_output(ctx)

        test_host_app_bundle = _get_test_host_app_bundle(ctx)
        test_host_app_binary = _get_test_host_app_binary(ctx, test_host_app_bundle)
        ui_test_target_app_bundle = _get_ui_test_target_app_bundle(ctx)

        objc_bridging_header_flags = (
            [
                # Disable bridging header -> PCH compilation to mitigate an issue in Xcode 13 beta.
                "-disable-bridging-pch",
                "-import-objc-header",
                cmd_args(ctx.attrs.bridging_header),
            ]
            if ctx.attrs.bridging_header
            else []
        )

        shared_library_flags = ["-bundle"]

        # Embedding entitlements (if present) means that we can skip adhoc codesigning
        # any xctests altogether, provided the test dylib is adhoc signed
        shared_library_flags += entitlements_link_flags(ctx) + get_rpath_flags_for_tests(ctx)

        # The linker will include adhoc signature for ARM64 by default, lets
        # ensure we always have an adhoc signature regardless of arch/linker logic.
        shared_library_flags += ["-Wl,-adhoc_codesign"]

        constructor_params = apple_library_rule_constructor_params_and_swift_providers(
            ctx,
            AppleLibraryAdditionalParams(
                rule_type = "apple_test",
                extra_exported_link_flags = get_test_framework_linker_flags(ctx) + _get_bundle_loader_flags(test_host_app_binary),
                extra_swift_compiler_flags = _get_test_framework_search_paths_flags(ctx) + objc_bridging_header_flags,
                shared_library_flags = SharedLibraryFlagOverrides(
                    # When `-bundle` is used we can't use the `-install_name` args, thus we keep this field empty.
                    shared_library_name_linker_flags_format = [],
                    # When building Apple tests, we want to link with `-bundle` instead of `-shared` to allow
                    # linking against the bundle loader.
                    shared_library_flags = shared_library_flags,
                ),
                generate_sub_targets = CxxRuleSubTargetParams(
                    compilation_database = True,
                    headers = False,
                    link_group_map = False,
                    xcode_data = xcode_data_enabled(),
                ),
                generate_providers = CxxRuleProviderParams(
                    compilation_database = True,
                    default = False,
                    linkable_graph = False,
                    link_style_outputs = True,
                    merged_native_link_info = False,
                    omnibus_root = False,
                    preprocessors = False,
                    resources = False,
                    shared_libraries = False,
                    template_placeholders = False,
                ),
                populate_xcode_attributes_func = lambda local_ctx, **kwargs: _xcode_populate_attributes(
                    ctx = local_ctx, xctest_bundle = xctest_bundle, test_host_app_binary = test_host_app_binary, test_host_app_bundle = test_host_app_bundle, **kwargs
                ),
                # We want to statically link the transitive dep graph of the apple_test()
                # which we can achieve by forcing link group linking with
                # an empty mapping (i.e., default mapping).
                force_link_group_linking = True,
            ),
            deps_providers,
            is_test_target = True,
        )

        cxx_library_output = cxx_library_parameterized(ctx, constructor_params)

        # Locate the temporary binary that is bundled into the xctest in a binaries directory. When Xcode loads the test out of the target's output dir,
        # it will utilize a binary with the test name from the output dir instead of the xctest bundle. Which then results in paths to test resources
        # being incorrect. Locating the temporary binary elsewhere works around this issue.
        test_binary_output = ctx.actions.declare_output("__binaries__", get_product_name(ctx), has_content_based_path = False)

        # Rename in order to generate dSYM with correct binary name (dsymutil doesn't provide a way to control binary name in output dSYM bundle).
        test_binary = ctx.actions.copy_file(test_binary_output, cxx_library_output.default_output.default)

        binary_part = AppleBundlePart(source = test_binary, destination = AppleBundleDestination("executables"), new_name = ctx.attrs.name)
        part_list_output = get_apple_bundle_part_list(ctx, AppleBundlePartListConstructorParams(binaries = [binary_part]))

        xctest_swift_support_needed = None
        debug_info = None
        cxx_providers = []
        for p in cxx_library_output.providers:
            if isinstance(p, XCTestSwiftSupportInfo):
                xctest_swift_support_needed = p.support_needed
            elif isinstance(p, AppleDebuggableInfo):
                debug_info = project_artifacts(ctx.actions, p.debug_info_tset)
            elif isinstance(p, TargetStatsInfo):
                cxx_providers.append(p)
            elif isinstance(p, ValidationInfo):
                cxx_providers.append(p)
        expect(xctest_swift_support_needed != None, "Expected `XCTestSwiftSupportInfo` provider to be present")
        expect(debug_info != None, "Expected `AppleDebuggableInfo` provider to be present")

        bundle_parts = part_list_output.parts
        if not ctx.attrs.embed_xctest_frameworks_in_test_host_app and get_apple_sdk_name(ctx) != WatchSimulatorSdkMetadata.name:
            # The XCTest frameworks should only be embedded in a single place,
            # either the test host (as per Xcode) or in the test itself
            if test_host_app_bundle != None or read_root_config("apple", "exclude_xctest_libraries", "false").lower() != "true":
                bundle_parts += get_test_frameworks_bundle_parts(ctx, xctest_swift_support_needed)

        for sanitizer_runtime_dylib in cxx_library_output.sanitizer_runtime_files:
            frameworks_destination = AppleBundleDestination("frameworks")
            bundle_parts.append(
                AppleBundlePart(
                    source = sanitizer_runtime_dylib,
                    destination = frameworks_destination,
                    codesign_on_copy = True,
                ),
            )

        bundle_result = assemble_bundle(
            ctx,
            xctest_bundle,
            bundle_parts,
            part_list_output.codesign_manifest_parts,
            part_list_output.signing_context_parts,
            part_list_output.info_plist_part,
            None,  # swift_stdlib_args
            # Adhoc signing can be skipped because the test executable is adhoc signed
            # + includes any entitlements if present.
            skip_adhoc_signing = True,
            incremental_bundling_override = False,
        )
        sub_targets = bundle_result.sub_targets
        sub_targets.update(cxx_library_output.sub_targets)

        dsym_artifact = get_apple_dsym(
            ctx = ctx,
            executable = test_binary,
            debug_info = debug_info,
            action_identifier = "generate_apple_test_dsym",
            output_path_override = get_bundle_dir_name(ctx) + ".dSYM",
        )
        sub_targets[DSYM_SUBTARGET] = [DefaultInfo(default_output = dsym_artifact)]

        deps_debuggable_infos = get_deps_debuggable_infos(ctx)
        dep_dsym_artifacts = []
        for debuggable_info in deps_debuggable_infos:
            dep_dsym_artifacts.extend(debuggable_info.dsyms)

        dsym_json_info = get_apple_dsym_info_json([dsym_artifact], dep_dsym_artifacts)
        dsym_info = ctx.actions.write_json("extended-dsym-info.json", dsym_json_info.json_object, pretty = True, has_content_based_path = False)
        sub_targets[EXTENDED_DSYM_INFO_SUBTARGET] = [
            DefaultInfo(default_output = dsym_info, other_outputs = dsym_json_info.outputs),
        ]

        # If the test has a test host and a ui test target, add the subtargets to build the app bundles.
        sub_targets["test-host"] = [DefaultInfo(default_output = test_host_app_bundle)] if test_host_app_bundle else [DefaultInfo()]
        sub_targets["ui-test-target"] = [DefaultInfo(default_output = ui_test_target_app_bundle)] if ui_test_target_app_bundle else [DefaultInfo()]

        sub_targets[DWARF_AND_DSYM_SUBTARGET] = [
            DefaultInfo(
                default_output = xctest_bundle, other_outputs = [dsym_artifact], sub_targets = {_XCTOOLCHAIN_SUB_TARGET: ctx.attrs._apple_xctoolchain.providers}
            ),
            _get_test_info(ctx, xctest_bundle, test_host_app_bundle, dsym_artifact, ui_test_target_app_bundle),
        ]

        sub_targets[_XCTOOLCHAIN_SUB_TARGET] = ctx.attrs._apple_xctoolchain.providers

        return (
            filter(
                None,
                [
                    DefaultInfo(default_output = xctest_bundle, sub_targets = sub_targets),
                    _get_test_info(ctx, xctest_bundle, test_host_app_bundle, ui_test_target_app_bundle = ui_test_target_app_bundle),
                    AppleBundleResourceInfo(resource_output = part_list_output.resource_output),
                    cxx_library_output.index_store_info,
                    cxx_library_output.xcode_data_info,
                    cxx_library_output.cxx_compilationdb_info,
                ],
            )
            + bundle_result.providers
            + cxx_providers
        )

    if uses_explicit_modules(ctx):
        return get_swift_anonymous_targets(ctx, get_apple_test_providers)
    else:
        return get_apple_test_providers([])

def _get_test_info(
    ctx: AnalysisContext,
    xctest_bundle: Artifact,
    test_host_app_bundle: Artifact | None,
    dsym_artifact: Artifact | None = None,
    ui_test_target_app_bundle: Artifact | None = None,
) -> Provider:
    # When interacting with Tpx, we just pass our various inputs via env vars,
    # since Tpx basically wants structured output for this.

    xctest_bundle = cmd_args(xctest_bundle, hidden = dsym_artifact) if dsym_artifact else xctest_bundle
    env = {"XCTEST_BUNDLE": xctest_bundle}

    if test_host_app_bundle == None:
        tpx_label = "tpx:apple_test:buck2:logicTest"
    else:
        env["HOST_APP_BUNDLE"] = test_host_app_bundle
        tpx_label = "tpx:apple_test:buck2:appTest"

    if ui_test_target_app_bundle != None:
        env["TARGET_APP_BUNDLE"] = ui_test_target_app_bundle
        tpx_label = "tpx:apple_test:buck2:uiTest"

    labels = ctx.attrs.labels
    labels.append(tpx_label)

    test_device_type = AppleTestDeviceType(ctx.attrs.test_device_type)
    if test_device_type == AppleTestDeviceType("default"):
        # determine the device type from the sdk and platform
        sdk_name = get_apple_sdk_name(ctx)
        test_device_type = get_default_test_device(sdk = sdk_name, platform = ctx.attrs.default_target_platform)
    labels.append(tpx_label_for_test_device_type(test_device_type))

    remote_execution_properties = None # @oss-enable
    remote_execution_use_case = None # @oss-enable

    # @oss-disable[end= ]: if ctx.attrs.test_re_capabilities:
        # @oss-disable[end= ]: remote_execution_properties = ctx.attrs.test_re_capabilities
    # @oss-disable[end= ]: else:
        # @oss-disable[end= ]: uses_test_host = test_host_app_bundle != None or ui_test_target_app_bundle != None
        # @oss-disable[end= ]: remote_execution_properties = apple_test_re_capabilities(test_device_type = test_device_type, uses_test_host = uses_test_host)
    # @oss-disable[end= ]: remote_execution_use_case = ctx.attrs.test_re_use_case or apple_test_re_use_case(test_device_type = test_device_type)

    # @oss-disable[end= ]: if local_test_execution_is_available():
        # @oss-disable[end= ]: labels.append("tpx:apple_test:local_execution_available")

    return ExternalRunnerTestInfo(
        type = "apple_test",
        command = ["false"],  # Tpx makes up its own args, we just pass params via the env.
        env = flatten_x([ctx.attrs.env or {}, env]),
        labels = labels,
        use_project_relative_paths = True,
        run_from_project_root = True,
        contacts = ctx.attrs.contacts,
        executor_overrides = {
            "ios-simulator-local": CommandExecutorConfig(
                local_enabled = True,
                remote_enabled = False,
                remote_execution_properties = None,
                remote_execution_use_case = None,
            ),
            "ios-simulator-remote": CommandExecutorConfig(
                local_enabled = False,
                remote_enabled = True,
                remote_execution_properties = remote_execution_properties,
                remote_execution_use_case = remote_execution_use_case,
            ),
            "static-listing": CommandExecutorConfig(local_enabled = True, remote_enabled = False),
        },
        local_resources = {
            "ipad_simulator": ctx.attrs._ipad_simulator.label,
            "iphone_booted_simulator": ctx.attrs._iphone_booted_simulator.label,
            "iphone_unbooted_simulator": ctx.attrs._iphone_unbooted_simulator.label,
            "watch_simulator": ctx.attrs._watch_simulator.label,
        },
    )

def _get_test_host_app_bundle(ctx: AnalysisContext) -> Artifact | None:
    """Get the bundle for the test host app, if one exists for this test."""
    if ctx.attrs.test_host_app:
        # Copy the test host app bundle into test's output directory
        original_bundle = ctx.attrs.test_host_app[AppleBundleInfo].bundle
        test_host_app_bundle = ctx.actions.declare_output(original_bundle.basename, has_content_based_path = False)
        ctx.actions.copy_file(test_host_app_bundle, original_bundle)
        return test_host_app_bundle

    return None

def _get_test_host_app_binary(ctx: AnalysisContext, test_host_app_bundle: Artifact | None) -> [cmd_args, None]:
    """Reference to the binary with the test host app bundle, if one exists for this test. Captures the bundle as an artifact in the cmd_args."""
    if ctx.attrs.test_host_app == None:
        return None

    parts = [test_host_app_bundle]
    rel_path = bundle_relative_path_for_destination(AppleBundleDestination("executables"), get_apple_sdk_name(ctx), ctx.attrs.extension, False)
    if len(rel_path) > 0:
        parts.append(rel_path)
    parts.append(ctx.attrs.test_host_app[AppleBundleInfo].binary_name)
    return cmd_args(parts, delimiter = "/")

def _get_ui_test_target_app_bundle(ctx: AnalysisContext) -> Artifact | None:
    """Get the bundle for the ui test target app, if one exists for this test."""
    if ctx.attrs.ui_test_target_app:
        # Copy the ui test target app bundle into test's output directory
        original_bundle = ctx.attrs.ui_test_target_app[AppleBundleInfo].bundle
        ui_test_target_app_bundle = ctx.actions.declare_output(original_bundle.basename, has_content_based_path = False)
        ctx.actions.copy_file(ui_test_target_app_bundle, original_bundle)
        return ui_test_target_app_bundle

    return None

def _get_bundle_loader_flags(binary: [cmd_args, None]) -> list[typing.Any]:
    if binary:
        # During linking we need to link the test shared lib against the test host binary. The
        # test host binary doesn't need to be embedded in an `apple_bundle`.
        return ["-bundle_loader", binary]

    return []

def _xcode_populate_attributes(
    ctx: AnalysisContext,
    srcs: list[CxxSrcWithFlags],
    argsfiles: dict[str, CompileArgsfile],
    xctest_bundle: Artifact,
    test_host_app_binary: [cmd_args, None],
    test_host_app_bundle: Artifact | None,
    **_kwargs,
) -> dict[str, typing.Any]:
    data = apple_populate_xcode_attributes(ctx = ctx, srcs = srcs, argsfiles = argsfiles, product_name = ctx.attrs.name)
    data[XcodeDataInfoKeys.OUTPUT] = xctest_bundle

    # TODO: We should be able to extract this information in BXL. XcodeData is primarily necessary for derived data from the rules.
    if ctx.attrs.ui_test_target_app:
        data[XcodeDataInfoKeys.TEST_TYPE] = "ui-test"
        data[XcodeDataInfoKeys.TEST_TARGET] = ctx.attrs.ui_test_target_app.label.raw_target()
    else:
        data[XcodeDataInfoKeys.TEST_TYPE] = "unit-test"
        if test_host_app_binary:
            data[XcodeDataInfoKeys.TEST_HOST_APP_BINARY] = test_host_app_binary
            data[XcodeDataInfoKeys.TEST_HOST_APP_BUNDLE] = test_host_app_bundle
            data[XcodeDataInfoKeys.TEST_HOST_APP_TARGET] = ctx.attrs.test_host_app.label.raw_target()
    return data

def _get_test_framework_search_paths(ctx: AnalysisContext) -> (cmd_args, cmd_args):
    toolchain = ctx.attrs._apple_toolchain[AppleToolchainInfo]
    test_swiftmodule_search_path = cmd_args([toolchain.platform_path, "Developer/usr/lib"], delimiter = "/")
    test_framework_search_path = cmd_args([toolchain.platform_path, "Developer/Library/Frameworks"], delimiter = "/")
    return (test_swiftmodule_search_path, test_framework_search_path)

def _get_test_framework_search_paths_flags(ctx: AnalysisContext) -> list[[cmd_args, str]]:
    test_swiftmodule_search_path, test_framework_search_path = _get_test_framework_search_paths(ctx)
    return [
        "-I",
        test_swiftmodule_search_path,
        "-F",
        test_framework_search_path,
    ]

def get_test_framework_linker_flags(ctx: AnalysisContext) -> list[[cmd_args, str]]:
    test_swiftmodule_search_path, test_framework_search_path = _get_test_framework_search_paths(ctx)
    linker_flags = [
        "-L",
        test_swiftmodule_search_path,
        "-F",
        test_framework_search_path,
    ]
    if ctx.attrs.swift_testing:
        linker_flags += ["-lXCTestSwiftSupport"]
    return linker_flags
