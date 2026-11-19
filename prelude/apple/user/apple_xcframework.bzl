# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//apple:apple_bundle_types.bzl", "AppleBundleInfo")
load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolsInfo")

# Metadata about XCFramework
XCFrameworkInfo = provider(
    fields = {
        # Name of supplied framework. For frameworks that define
        # modules this should be the name of the module
        "name": provider_field(str),
    },
)

def _get_any_framework_dep(framework_deps: [dict[str, Dependency], Dependency, None]) -> [Dependency, None]:
    if not type(framework_deps) == "dict":
        return framework_deps
    if len(framework_deps) == 0:
        return None
    return sorted(framework_deps.items())[0][1]

def _get_framework_name(ctx: AnalysisContext) -> str:
    if ctx.attrs.framework_name != None:
        return ctx.attrs.framework_name
    if ctx.attrs.framework_name_from_product_name:
        framework = _get_any_framework_dep(ctx.attrs.framework)
        if framework == None:
            fail("Cannot find framework name")
        return framework[AppleBundleInfo].binary_name
    fail("Cannot find framework name")

def apple_xcframework_impl(ctx: AnalysisContext) -> list[Provider]:
    apple_tools = ctx.attrs._apple_tools[AppleToolsInfo]
    framework_name = _get_framework_name(ctx)

    xcframework_dir = ctx.actions.declare_output(framework_name + ".xcframework", dir = True, has_content_based_path = False)
    xcframework_command = cmd_args([
        apple_tools.xcframework_maker,
        "--output-path",
        xcframework_dir.as_output(),
        "--name",
        framework_name,
    ])

    for arch in ctx.attrs.framework:
        framework_dep = ctx.attrs.framework[arch]
        framework_paths = framework_dep[DefaultInfo].default_outputs
        if len(framework_paths) > 1:
            fail("xcframework's framework target {} must only produce one output".format(framework_dep.label))

        xcframework_command.add("--framework-path")
        xcframework_command.add(arch)
        xcframework_command.add(framework_paths[0])

        if ctx.attrs.include_dsym:
            dsym_dep = framework_dep[DefaultInfo].sub_targets["dsym"]
            dsym_path = dsym_dep[DefaultInfo].default_outputs
            xcframework_command.add("--dsym-path")
            xcframework_command.add(arch)
            xcframework_command.add(dsym_path)

    ctx.actions.run(xcframework_command, category = "apple_xcframework")
    return [
        DefaultInfo(default_output = xcframework_dir),
        XCFrameworkInfo(name = framework_name),
    ]

def _strip_os_sdk_and_runtime_constraints(platform: PlatformInfo, refs: struct) -> dict[TargetLabel, ConstraintValueInfo]:
    return {
        constraint_setting_label: constraint_setting_value
        for (constraint_setting_label, constraint_setting_value) in platform.configuration.constraints.items()
        if constraint_setting_label
        not in [
            refs.os[ConstraintSettingInfo].label,
            refs.sdk[ConstraintSettingInfo].label,
            refs.universal[ConstraintSettingInfo].label,
            refs.runtime[ConstraintSettingInfo].label,
        ]
    }

# provides a map of os-platform to cpu architectures
# so we can identify when universal binaries can be created instead of
# two separate frameworks
#
# e.g. input of ["ios-arm64", "iphonesimulator-x86_64", "iphonesimulator-arm64"]
# will produce {"ios": ["arm64"], "iphonesimulator": ["arm64", "x86_64"]}

def _normalize_platforms(platforms: list[str]) -> dict[str, list[str]]:
    result = {}
    for platform in platforms:
        plat_list = platform.split("-")
        plat_type = plat_list[0]
        plat_archs = plat_list[1:]
        previous_archs = result.get(plat_type, [])
        result[plat_type] = sorted(plat_archs + previous_archs)

    return result

def _apple_xcframework_framework_attrib_split_transition_impl(platform: PlatformInfo, refs: struct, attrs: struct) -> dict[str, PlatformInfo]:
    result = {}

    new_platforms = _normalize_platforms(attrs.platforms).items()
    for os_value, cpu_values in new_platforms:
        updated_constraints = _strip_os_sdk_and_runtime_constraints(platform, refs)

        canonical_platform_suffix = ""

        if os_value == "macos":
            canonical_platform_prefix = "macos"
            updated_constraints[refs.os[ConstraintSettingInfo].label] = refs.macos[ConstraintValueInfo]
        elif os_value == "iphoneos":
            canonical_platform_prefix = "ios"
            updated_constraints[refs.os[ConstraintSettingInfo].label] = refs.ios[ConstraintValueInfo]
            updated_constraints[refs.sdk[ConstraintSettingInfo].label] = refs.ios_device_sdk[ConstraintValueInfo]
        elif os_value == "watchos":
            canonical_platform_prefix = "watchos"
            updated_constraints[refs.os[ConstraintSettingInfo].label] = refs.watchos[ConstraintValueInfo]
            updated_constraints[refs.sdk[ConstraintSettingInfo].label] = refs.watchos_device_sdk[ConstraintValueInfo]
        elif os_value == "iphonesimulator":
            canonical_platform_prefix = "ios"
            canonical_platform_suffix = "simulator"
            updated_constraints[refs.os[ConstraintSettingInfo].label] = refs.ios[ConstraintValueInfo]
            updated_constraints[refs.sdk[ConstraintSettingInfo].label] = refs.ios_simulator_sdk[ConstraintValueInfo]
        elif os_value == "watchossimulator":
            canonical_platform_prefix = "watchos"
            canonical_platform_suffix = "simulator"
            updated_constraints[refs.os[ConstraintSettingInfo].label] = refs.watchos[ConstraintValueInfo]
            updated_constraints[refs.sdk[ConstraintSettingInfo].label] = refs.watchos_simulator_sdk[ConstraintValueInfo]
        elif os_value == "maccatalyst":
            canonical_platform_prefix = "ios"
            canonical_platform_suffix = "maccatalyst"
            updated_constraints[refs.os[ConstraintSettingInfo].label] = refs.ios[ConstraintValueInfo]
            updated_constraints[refs.sdk[ConstraintSettingInfo].label] = refs.maccatalyst_sdk[ConstraintValueInfo]
            updated_constraints[refs.runtime[ConstraintSettingInfo].label] = refs.maccatalyst_runtime[ConstraintValueInfo]
        else:
            fail("Unsupported OS value {} in apple_xcframework() platforms.".format(os_value))

        cpu_constraint_name = refs.cpu[ConstraintSettingInfo].label

        if len(cpu_values) > 1:
            updated_constraints[refs.universal[ConstraintSettingInfo].label] = refs.universal_enabled[ConstraintValueInfo]
        elif cpu_values[0] == "arm64":
            updated_constraints[cpu_constraint_name] = refs.arm64[ConstraintValueInfo]
        elif cpu_values[0] == "x86_64":
            updated_constraints[cpu_constraint_name] = refs.x86_64[ConstraintValueInfo]
        else:
            fail("Unsupported CPU value {} in apple_xcframework().".format(cpu_values[0]))

        new_cfg = ConfigurationInfo(
            constraints = updated_constraints,
            values = platform.configuration.values,
        )

        canonical_platform_name = canonical_platform_prefix + "-" + "_".join(cpu_values)
        if len(canonical_platform_suffix) > 0:
            canonical_platform_name += "-" + canonical_platform_suffix

        result.update({
            canonical_platform_name: PlatformInfo(
                label = canonical_platform_name + "_transition",
                configuration = new_cfg,
            )
        })

    return result

framework_split_transition = transition(
    impl = _apple_xcframework_framework_attrib_split_transition_impl,
    refs = {
        "arm32": "config//cpu/constraints:arm32",
        "arm64": "config//cpu/constraints:arm64",
        "cpu": "config//cpu/constraints:cpu",
        "ios": "config//os/constraints:iphoneos",
        "ios_device_sdk": "config//os/sdk/apple/constraints:iphoneos",
        "ios_simulator_sdk": "config//os/sdk/apple/constraints:iphonesimulator",
        "maccatalyst_runtime": "config//runtime/constraints:maccatalyst",
        "maccatalyst_sdk": "config//os/sdk/apple/constraints:maccatalyst",
        "macos": "config//os/constraints:macos",
        "os": "config//os/constraints:os",
        "runtime": "config//runtime/constraints:runtime",
        "sdk": "config//os/sdk/apple/constraints:_",
        "swift_library_evolution": "config//features/apple/constraints:swift_library_evolution",
        "swift_library_evolution_enabled": "config//features/apple/constraints:swift_library_evolution_enabled",
        "universal": "config//cpu/constraints:universal",
        "universal_enabled": "config//cpu/constraints:universal-enabled",
        "watchos": "config//os/constraints:watchos",
        "watchos_device_sdk": "config//os/sdk/apple/constraints:watchos",
        "watchos_simulator_sdk": "config//os/sdk/apple/constraints:watchsimulator",
        "x86_64": "config//cpu/constraints:x86_64",
    },
    attrs = [
        "platforms",
    ],
    split = True,
)
