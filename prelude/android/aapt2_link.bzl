# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//android:android_providers.bzl", "Aapt2LinkInfo", "AndroidResourceInfo", "RESOURCE_PRIORITY_LOW")
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")

BASE_PACKAGE_ID = 0x7F

def normalize_locale(locale: str) -> str:
    if locale == "NONE":
        return "en"
    if regex("^[a-z][a-z]$").match(locale):
        return locale
    if regex("^[a-z][a-z]_[A-Z][A-Z]$").match(locale):
        # This transforms locales from xx_YY to xx-rYY.
        # E.g.: like "zh_CN" to "zh-rCN".
        return locale[:2] + "-r" + locale[-2:]
    if regex("^[a-z][a-z]-r[A-Z][A-Z]$").match(locale):
        # Already in xx-rYY format
        return locale
    fail("Invalid locale format passed: {} {}".format(locale, regex("/^[a-z][a-z]$/").match(locale)))

def get_aapt2_link(
    ctx: AnalysisContext,
    android_toolchain: AndroidToolchainInfo,
    resource_infos: list[AndroidResourceInfo],
    android_manifest: Artifact,
    manifest_entries: dict,
    includes_vector_drawables: bool,
    no_resource_removal: bool,
    should_keep_raw_values: bool,
    package_id_offset: int,
    resource_stable_ids: Artifact | None,
    preferred_density: [str, None],
    filter_locales: bool,
    locales: list[str],
    compiled_resource_apks: list[Artifact],
    additional_aapt2_params: list[str],
    extra_filtered_resources: list[str],
) -> (Aapt2LinkInfo, Aapt2LinkInfo):
    link_infos = []
    for use_proto_format in [False, True]:
        if use_proto_format:
            identifier = "use_proto"
        else:
            identifier = "not_proto"

        aapt2_command = cmd_args(android_toolchain.aapt2)
        aapt2_command.add("link")

        # aapt2 only supports @ for -R or input files, not for all args, so we pass in all "normal"
        # args here.
        resources_apk = ctx.actions.declare_output("{}/resource-apk.ap_".format(identifier), has_content_based_path = False)
        aapt2_command.add(["-o", resources_apk.as_output()])
        proguard_config = ctx.actions.declare_output("{}/proguard_config.pro".format(identifier), has_content_based_path = False)
        aapt2_command.add(["--proguard", proguard_config.as_output()])

        # We don't need the R.java output, but aapt2 won't output R.txt unless we also request R.java.
        # A drawback of this is that the directory structure for the R.java output is deep, resulting
        # in long path issues on Windows. The structure is <path to target>/<identifier>/unused-rjava/<package>/R.java
        # We can declare a custom dummy package to drastically shorten <package>, which is sketchy, but effective
        r_dot_java = ctx.actions.declare_output("{}/unused-rjava".format(identifier), dir = True, has_content_based_path = False)
        aapt2_command.add(["--java", r_dot_java.as_output()])
        aapt2_command.add(["--custom-package", "dummy.package"])

        r_dot_txt = ctx.actions.declare_output("{}/R.txt".format(identifier), has_content_based_path = False)
        aapt2_command.add(["--output-text-symbols", r_dot_txt.as_output()])

        aapt2_command.add(["--manifest", android_manifest])
        aapt2_command.add(["-I", android_toolchain.android_jar])

        if includes_vector_drawables:
            aapt2_command.add("--no-version-vectors")
        aapt2_command.add("--auto-add-overlay")
        if use_proto_format:
            aapt2_command.add("--proto-format")
        if no_resource_removal:
            aapt2_command.add("--no-resource-removal")
        if should_keep_raw_values:
            aapt2_command.add("--keep-raw-values")
        if package_id_offset != 0:
            aapt2_command.add(["--package-id", "0x{}".format(BASE_PACKAGE_ID + package_id_offset)])
        if resource_stable_ids != None:
            aapt2_command.add(["--stable-ids", resource_stable_ids])
        if preferred_density != None:
            aapt2_command.add(["--preferred-density", preferred_density])

        manifest_entries_min_sdk = manifest_entries.get("min_sdk_version", None)
        if manifest_entries_min_sdk != None:
            aapt2_command.add(["--min-sdk-version", str(manifest_entries_min_sdk)])
        manifest_entries_target_sdk = manifest_entries.get("target_sdk_version", None)
        if manifest_entries_target_sdk != None:
            aapt2_command.add(["--target-sdk-version", str(manifest_entries_target_sdk)])
        manifest_entries_version_code = manifest_entries.get("version_code", None)
        if manifest_entries_version_code != None:
            aapt2_command.add(["--version-code", manifest_entries_version_code])
        manifest_entries_version_name = manifest_entries.get("version_name", None)
        if manifest_entries_version_name != None:
            aapt2_command.add(["--version-name", manifest_entries_version_name])
        manifest_entries_debug_mode = str(manifest_entries.get("debug_mode", "False")).lower() == "true"
        if manifest_entries_debug_mode:
            aapt2_command.add(["--debug-mode"])

        if filter_locales and len(locales) > 0:
            for locale in locales:
                aapt2_command.add(["-c", normalize_locale(locale)])

        for compiled_resource_apk in compiled_resource_apks:
            aapt2_command.add(["-I", compiled_resource_apk])

        # put low priority resources first so that they get overwritten by higher priority resources
        low_priority_aapt2_compile_rules = []
        normal_priority_aapt2_compile_rules = []
        for resource_info in resource_infos:
            if resource_info.aapt2_compile_output:
                (low_priority_aapt2_compile_rules if resource_info.res_priority == RESOURCE_PRIORITY_LOW else normal_priority_aapt2_compile_rules).append(
                    resource_info.aapt2_compile_output
                )
        aapt2_compile_rules = low_priority_aapt2_compile_rules + normal_priority_aapt2_compile_rules

        aapt2_compile_rules_args_file = ctx.actions.write(
            "{}/aapt2_compile_rules_args_file".format(identifier), cmd_args(aapt2_compile_rules, delimiter = " "), has_content_based_path = False
        )
        aapt2_command.add("-R")
        aapt2_command.add(
            cmd_args(
                aapt2_compile_rules_args_file,
                format = "@{}",
                hidden = aapt2_compile_rules,
            )
        )

        aapt2_command.add(additional_aapt2_params)

        ctx.actions.run(aapt2_command, category = "aapt2_link", identifier = identifier, allow_cache_upload = True, error_handler = aapt_link_error_handler)

        # The normal resource filtering apparatus is super slow, because it extracts the whole apk,
        # strips files out of it, then repackages it.
        #
        # This is a faster filtering step that just uses zip -d to remove entries from the archive.
        # It's also superbly dangerous.
        #
        # If zip -d returns that there was nothing to do, then we don't fail.
        if len(extra_filtered_resources) > 0:
            filtered_resources_apk = ctx.actions.declare_output("{}/filtered-resource-apk.ap_".format(identifier), has_content_based_path = False)
            filter_resources_cmd = cmd_args(ctx.attrs._android_toolchain[AndroidToolchainInfo].aapt2_filter_resources)
            filter_resources_cmd.add(cmd_args(resources_apk, format = "--input-apk={}"))
            filter_resources_cmd.add(cmd_args(filtered_resources_apk.as_output(), format = "--output-apk={}"))
            filter_resources_cmd.add(cmd_args(extra_filtered_resources, format = "--extra-filtered-resources={}"))
            ctx.actions.run(filter_resources_cmd, category = "aapt2_filter_resources", identifier = identifier, allow_cache_upload = True)
            primary_resources_apk = filtered_resources_apk
        else:
            primary_resources_apk = resources_apk

        link_infos.append(
            Aapt2LinkInfo(
                primary_resources_apk = primary_resources_apk,
                proguard_config_file = proguard_config,
                r_dot_txt = r_dot_txt,
            )
        )

    return link_infos[0], link_infos[1]

def get_module_manifest_in_proto_format(
    ctx: AnalysisContext, android_toolchain: AndroidToolchainInfo, android_manifest: Artifact, primary_resources_apk: Artifact, module_name: str
) -> Artifact:
    aapt2_command = cmd_args(android_toolchain.aapt2)
    aapt2_command.add("link")

    # aapt2 only supports @ for -R or input files, not for all args, so we pass in all "normal"
    # args here.
    resources_apk = ctx.actions.declare_output("{}/resource-apk.ap_".format(module_name), has_content_based_path = False)
    aapt2_command.add(["-o", resources_apk.as_output()])
    aapt2_command.add(["--manifest", android_manifest])
    aapt2_command.add(["-I", android_toolchain.android_jar])
    aapt2_command.add(["-I", primary_resources_apk])
    aapt2_command.add("--proto-format")

    ctx.actions.run(aapt2_command, category = "aapt2_link", identifier = module_name, allow_cache_upload = True, error_handler = aapt_link_error_handler)

    proto_manifest_dir = ctx.actions.declare_output("{}/proto_format_manifest".format(module_name), has_content_based_path = False)
    proto_manifest = proto_manifest_dir.project("AndroidManifest.xml")
    ctx.actions.run(
        cmd_args(["unzip", resources_apk, "AndroidManifest.xml", "-d", proto_manifest_dir.as_output()]),
        category = "unzip_proto_format_manifest",
        identifier = module_name,
        allow_cache_upload = True,
    )

    return proto_manifest

def aapt_link_error_handler(ctx: ActionErrorCtx) -> list[ActionSubError]:
    errors = []
    lowercase_stderr = ctx.stderr.lower()
    if regex(r"failed to write .* to archive").match(lowercase_stderr):
        errors.append(
            ctx.new_sub_error(
                category = "aapt2_link",
                message = "Most probably there are too many res files in the apk (over 65535). Try filtering out some resources.",
                show_in_stderr = True,
            )
        )
    return errors
