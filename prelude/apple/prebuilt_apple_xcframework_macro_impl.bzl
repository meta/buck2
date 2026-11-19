# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//apple:arch.bzl", "AppleArch")
load("@prelude//utils:glob_defs.bzl", "subdir_glob")
load("@prelude//utils:selects.bzl", "selects")

def _generate_framework_and_dsym_select_maps(prebuilt_xcframework_args, platform_filter, default_arch, default_target_platform, genrule):
    """
    Generate genrules for frameworks and dsyms, and return select maps for the attributes.
    Returns a tuple of (framework_select_map, dsym_select_map) where each maps config conditions to targets/artifacts.
    """
    name = prebuilt_xcframework_args.name
    platforms = prebuilt_xcframework_args.platforms
    xcframework = prebuilt_xcframework_args.xcframework
    framework_name = prebuilt_xcframework_args.framework_name
    framework_platforms_to_dsym_filenames = prebuilt_xcframework_args.framework_platforms_to_dsym_filenames

    framework_select_map = {}
    dsym_select_map = {}

    for platform in platforms:
        if platform_filter(platform):
            bash_cmd = "mkdir $OUT && cp -R $(location {})/{}/{}.framework/* $OUT/".format(xcframework, platform["folder"], framework_name)
            assemble_name = "{}-{}-assemble".format(name, platform["folder"])
            genrule(
                name = assemble_name,
                bash = bash_cmd,
                out = framework_name + ".framework",
                default_target_platform = default_target_platform,
                has_content_based_path = True,
            )

            dsym_artifacts = selects.apply(
                framework_platforms_to_dsym_filenames,
                partial(_generate_dsym_artifacts, name, platform["folder"], xcframework, default_target_platform, genrule),
            )

            for arch in platform["archs"]:
                if arch not in ["arm64", "arm64e", "x86_64"]:
                    fail("Unsupported " + platform["platform"] + " arch: " + arch)

                config_key = "ovr_config//cpu:" + arch
                framework_select_map[config_key] = ":" + assemble_name
                dsym_select_map[config_key] = dsym_artifacts

                # We want to set DEFAULT if
                #  - We have specified a default_arch and it matches
                #  - We haven't specified a default_arch in which case
                #    we take the first match
                if (default_arch != None and AppleArch(arch) == default_arch) or framework_select_map.get("DEFAULT") == None:
                    framework_select_map["DEFAULT"] = ":" + assemble_name
                    dsym_select_map["DEFAULT"] = dsym_artifacts

    return (framework_select_map, dsym_select_map)

def _generate_dsym_artifacts(name, platform_folder, xcframework, default_target_platform, genrule, framework_platforms_to_dsym_filenames):
    return [
        _generate_dsym_artifact(name, platform_folder, xcframework, default_target_platform, dsym_filename, genrule)
        for dsym_filename in framework_platforms_to_dsym_filenames.get(platform_folder, [])
    ]

def _generate_dsym_artifact(name, platform_folder, xcframework, default_target_platform, dsym_filename, genrule):
    bash_cmd_dsyms = "mkdir $OUT && cp -R $(location {})/{}/dSYMs/{}/* $OUT/".format(xcframework, platform_folder, dsym_filename)
    dsym_name = "{}-{}-dsym".format(name, platform_folder)
    genrule(
        name = dsym_name,
        bash = bash_cmd_dsyms,
        out = dsym_filename,
        default_target_platform = default_target_platform,
        has_content_based_path = True,
    )
    return ":" + dsym_name

def _generate_iphonesimulator_select_maps(prebuilt_xcframework_args, **kwargs):
    return _generate_framework_and_dsym_select_maps(
        prebuilt_xcframework_args = prebuilt_xcframework_args,
        platform_filter = lambda platform: platform["platform"] == "ios" and platform["isSimulator"] and not platform["isCatalyst"],
        default_arch = AppleArch("arm64"),
        default_target_platform = "ovr_config//platform/iphoneos:iphonesimulator-arm64",
        **kwargs,
    )

def _generate_iphone_select_maps(prebuilt_xcframework_args, **kwargs):
    return _generate_framework_and_dsym_select_maps(
        prebuilt_xcframework_args = prebuilt_xcframework_args,
        platform_filter = lambda platform: platform["platform"] == "ios" and not platform["isSimulator"] and not platform["isCatalyst"],
        default_arch = None,
        default_target_platform = "ovr_config//platform/iphoneos:iphoneos-arm64",
        **kwargs,
    )

def _generate_mac_select_maps(prebuilt_xcframework_args, **kwargs):
    return _generate_framework_and_dsym_select_maps(
        prebuilt_xcframework_args = prebuilt_xcframework_args,
        platform_filter = lambda platform: platform["platform"] == "macos",
        default_arch = None,
        default_target_platform = "ovr_config//platform/macos:arm64-fbsource",
        **kwargs,
    )

def _generate_maccatalyst_select_maps(prebuilt_xcframework_args, **kwargs):
    return _generate_framework_and_dsym_select_maps(
        prebuilt_xcframework_args = prebuilt_xcframework_args,
        platform_filter = lambda platform: platform["platform"] == "ios" and not platform["isSimulator"] and platform["isCatalyst"],
        default_arch = None,
        default_target_platform = "ovr_config//platform/macos:arm64-catalyst",
        **kwargs,
    )

def prebuilt_apple_xcframework_macro_impl(
    prebuilt_apple_framework_rule,
    filegroup_rule,
    name,
    xcframework,
    framework_name,
    framework_platforms,
    framework_platforms_to_dsym_filenames = {},
    default_target_platform = None,
    **kwargs,
):
    compatible_with = kwargs.pop("compatible_with", None)

    parsed_platforms = [_parse_platform(platform) for platform in framework_platforms]

    # The xcframework is not required to be on disk, so we must make the folder path compatible with getting it from another target
    if ":" in xcframework:
        xcframework_target = xcframework
    else:
        xcframework_target = ":" + xcframework
        filegroup_rule(
            name = xcframework,
            srcs = subdir_glob(
                [
                    (xcframework, "**/*"),
                ],
            ),
        )

    prebuilt_xcframework_args = struct(
        name = name,
        platforms = parsed_platforms,
        xcframework = xcframework_target,
        framework_name = framework_name,
        framework_platforms_to_dsym_filenames = framework_platforms_to_dsym_filenames,
    )

    genrule = kwargs.pop("genrule", None)
    if genrule == None:
        fail("genrule must be provided")

    # Generate framework/dsym genrules and get select maps for each platform
    iphonesimulator_framework_map, iphonesimulator_dsym_map = _generate_iphonesimulator_select_maps(prebuilt_xcframework_args, genrule = genrule)
    iphone_framework_map, iphone_dsym_map = _generate_iphone_select_maps(prebuilt_xcframework_args, genrule = genrule)
    mac_framework_map, mac_dsym_map = _generate_mac_select_maps(prebuilt_xcframework_args, genrule = genrule)
    maccatalyst_framework_map, maccatalyst_dsym_map = _generate_maccatalyst_select_maps(prebuilt_xcframework_args, genrule = genrule)

    # Determine default platform's maps (priority: iphonesimulator > iphone > mac > maccatalyst)
    default_framework_map = None
    default_dsym_map = None

    if len(iphonesimulator_framework_map) > 0:
        default_framework_map = iphonesimulator_framework_map
        default_dsym_map = iphonesimulator_dsym_map
    elif len(iphone_framework_map) > 0:
        default_framework_map = iphone_framework_map
        default_dsym_map = iphone_dsym_map
    elif len(mac_framework_map) > 0:
        default_framework_map = mac_framework_map
        default_dsym_map = mac_dsym_map
    elif len(maccatalyst_framework_map) > 0:
        default_framework_map = maccatalyst_framework_map
        default_dsym_map = maccatalyst_dsym_map

    if default_framework_map == None:
        fail("No valid platforms found in xcframework")

    # Build combined maps for iphonesimulator+maccatalyst (used for ovr_config//os:iphoneos and iphonesimulator SDK)
    iphonesimulator_and_maccatalyst_framework_map = iphonesimulator_framework_map
    iphonesimulator_and_maccatalyst_dsym_map = iphonesimulator_dsym_map
    if len(maccatalyst_framework_map) > 0:
        if len(iphonesimulator_framework_map) > 0:
            iphonesimulator_and_maccatalyst_framework_map = {
                "DEFAULT": select(iphonesimulator_framework_map),
                "ovr_config//runtime/constraints:maccatalyst": select(maccatalyst_framework_map),
            }
            iphonesimulator_and_maccatalyst_dsym_map = {
                "DEFAULT": select(iphonesimulator_dsym_map),
                "ovr_config//runtime/constraints:maccatalyst": select(maccatalyst_dsym_map),
            }
        else:
            iphonesimulator_and_maccatalyst_framework_map = maccatalyst_framework_map
            iphonesimulator_and_maccatalyst_dsym_map = maccatalyst_dsym_map

    # Build combined maps for iphone+maccatalyst (used for iphoneos SDK)
    iphone_and_maccatalyst_framework_map = iphone_framework_map
    iphone_and_maccatalyst_dsym_map = iphone_dsym_map
    if len(maccatalyst_framework_map) > 0:
        if len(iphone_framework_map) > 0:
            iphone_and_maccatalyst_framework_map = {
                "DEFAULT": select(iphone_framework_map),
                "ovr_config//runtime/constraints:maccatalyst": select(maccatalyst_framework_map),
            }
            iphone_and_maccatalyst_dsym_map = {
                "DEFAULT": select(iphone_dsym_map),
                "ovr_config//runtime/constraints:maccatalyst": select(maccatalyst_dsym_map),
            }
        else:
            iphone_and_maccatalyst_framework_map = maccatalyst_framework_map
            iphone_and_maccatalyst_dsym_map = maccatalyst_dsym_map

    # Build the nested select for framework attribute
    framework_select_map = {"DEFAULT": select(default_framework_map)}
    dsym_select_map = {"DEFAULT": select(default_dsym_map)}

    if len(iphonesimulator_and_maccatalyst_framework_map) > 0:
        framework_select_map["ovr_config//os:iphoneos"] = select(iphonesimulator_and_maccatalyst_framework_map)
        framework_select_map["ovr_config//os/sdk/apple:iphonesimulator"] = select(iphonesimulator_and_maccatalyst_framework_map)
        dsym_select_map["ovr_config//os:iphoneos"] = select(iphonesimulator_and_maccatalyst_dsym_map)
        dsym_select_map["ovr_config//os/sdk/apple:iphonesimulator"] = select(iphonesimulator_and_maccatalyst_dsym_map)

    if len(iphone_and_maccatalyst_framework_map) > 0:
        framework_select_map["ovr_config//os/sdk/apple:iphoneos"] = select(iphone_and_maccatalyst_framework_map)
        dsym_select_map["ovr_config//os/sdk/apple:iphoneos"] = select(iphone_and_maccatalyst_dsym_map)

    if len(mac_framework_map) > 0:
        framework_select_map["ovr_config//os:macos"] = select(mac_framework_map)
        dsym_select_map["ovr_config//os:macos"] = select(mac_dsym_map)

    # Create a single prebuilt_apple_framework with select() for framework and dsyms
    prebuilt_apple_framework_rule(
        name = name,
        framework = select(framework_select_map),
        dsyms = select(dsym_select_map),
        default_target_platform = default_target_platform,
        compatible_with = compatible_with,
        **kwargs,
    )

def _parse_platform(name):
    # parse metadata from conventional xcframework folders
    # produced by xcodebuild -create-xcframework
    #
    # examples:
    #
    # ios-arm64
    # ios-arm64_x86_64-simulator
    # ios-arm64_x86_64-maccatalyst
    # macos-arm64_x86_64
    substrings = name.split("-")
    isCatalyst = False
    isSimulator = False
    if len(substrings) > 2:
        isSimulator = substrings[2] == "simulator"
        isCatalyst = substrings[2] == "maccatalyst"
    archs = []
    remaining = substrings[1]
    for arch in ["arm64e", "arm64_32", "arm64", "x86_64"]:
        if arch in remaining:
            archs.append(arch)
            remaining = remaining.replace(arch, "")
    if not archs:
        fail("Failed to parse architectures for xcframework platform " + name)

    return {
        "archs": archs,
        "folder": name,
        "isCatalyst": isCatalyst,
        "isSimulator": isSimulator,
        "platform": substrings[0],
    }
