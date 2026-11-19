# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:genrule.bzl", "genrule_attributes", "process_genrule")
load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//android:android_apk.bzl", "get_install_info")
load(
    "@prelude//android:android_providers.bzl",
    "AndroidAabInfo",
    "AndroidApkInfo",
    "AndroidApkUnderTestInfo",
    "AndroidDerivedApkInfo",
    "AndroidPreprocessedJavaClassesInfo",
    "KeystoreInfo",
)
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:bundletool_util.bzl", "derive_universal_apk")
load("@prelude//android:native_build_commands.bzl", "GATORADE_PHASE_SUBTARGETS")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:genrule_common.bzl", "genrule_common")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//java:class_to_srcs.bzl", "JavaClassToSourceMapInfo")
load("@prelude//utils:expect.bzl", "expect")

# Native-library debug sub-targets forwarded through the apk_genrule wrapper only if present on the
# wrapped android_apk/android_aab. These exist only in certain configurations (e.g. native_merge_debug
# requires the relinker or native merging); relinked_libs and relinked_libs_manifest are always
# present on an android_binary, and empty unless it defers a relink. Always-present
# sub-targets like native_libs, linker_commands and native_build_commands are forwarded in the
# required set below.
_OPTIONAL_NATIVE_LIB_SUBTARGETS = [
    "gatorade_phase_evidence",
    "native_merge_debug",
    "relinked_libs",
    "relinked_libs_manifest",
    "relinker_extra_outputs",
    "unrelinked_libs",
]

def _forward_optional_native_lib_subtargets(input_subtargets: dict) -> dict:
    return {name: [input_subtargets[name][DefaultInfo]] for name in _OPTIONAL_NATIVE_LIB_SUBTARGETS if name in input_subtargets}

# The top-level Gatorade-phase sub-targets (`TARGET[early_gatorade]` etc.) are always present on the
# wrapped android_apk/android_aab, so forward them in the required set at every wrapper site.
def _forward_gatorade_phase_subtargets(input_subtargets: dict) -> dict:
    return {name: [input_subtargets[name][DefaultInfo]] for name in GATORADE_PHASE_SUBTARGETS}

def apk_genrule_impl(ctx: AnalysisContext) -> list[Provider]:
    expect((ctx.attrs.apk == None) != (ctx.attrs.aab == None), "Exactly one of 'apk' and 'aab' must be specified")

    input_android_apk_under_test_info = None
    input_android_apk_subtargets = None
    input_android_apk_template_placeholder_info = None
    input_android_aab_subtargets = None
    input_preprocessed_java_classes_info = None
    if ctx.attrs.apk != None:
        # TODO(T104150125) The underlying APK should not have exopackage enabled
        input_android_apk_info = ctx.attrs.apk[AndroidApkInfo]
        expect(input_android_apk_info != None, "'apk' attribute must be an Android APK!")
        input_apk = input_android_apk_info.apk
        input_manifest = input_android_apk_info.manifest
        input_materialized_artifacts = input_android_apk_info.materialized_artifacts
        input_unstripped_shared_libraries = input_android_apk_info.unstripped_shared_libraries
        input_android_apk_under_test_info = ctx.attrs.apk[AndroidApkUnderTestInfo]
        input_android_apk_subtargets = ctx.attrs.apk[DefaultInfo].sub_targets
        input_android_apk_template_placeholder_info = ctx.attrs.apk[TemplatePlaceholderInfo].keyed_variables
        if AndroidPreprocessedJavaClassesInfo in ctx.attrs.apk:
            input_preprocessed_java_classes_info = ctx.attrs.apk[AndroidPreprocessedJavaClassesInfo]

        env_vars = {
            "APK": cmd_args(input_apk),
        }
    else:
        input_android_aab_info = ctx.attrs.aab[AndroidAabInfo]
        expect(input_android_aab_info != None, "'aab' attribute must be an Android Bundle!")

        # It's not an APK, but buck1 does this so we do it too for compatibility
        input_apk = input_android_aab_info.aab
        input_manifest = input_android_aab_info.manifest
        input_materialized_artifacts = input_android_aab_info.materialized_artifacts
        input_android_aab_subtargets = ctx.attrs.aab[DefaultInfo].sub_targets
        input_unstripped_shared_libraries = input_android_aab_info.unstripped_shared_libraries
        if AndroidPreprocessedJavaClassesInfo in ctx.attrs.aab:
            input_preprocessed_java_classes_info = ctx.attrs.aab[AndroidPreprocessedJavaClassesInfo]

        env_vars = {
            "AAB": cmd_args(input_apk),
        }

    genrule_providers = process_genrule(
        ctx,
        ctx.attrs.out,
        ctx.attrs.outs,
        env_vars,
        other_outputs = input_materialized_artifacts,
        genrule_error_handler = ctx.attrs._android_toolchain[AndroidToolchainInfo].android_error_handler,
    )

    genrule_default_info = filter(lambda x: isinstance(x, DefaultInfo), genrule_providers)

    expect(
        len(genrule_default_info) == 1,
        "Expecting a single DefaultInfo, but got {}",
        genrule_default_info,
    )

    genrule_default_output = genrule_default_info[0].default_outputs[0]
    genrule_default_output_is_aab = genrule_default_output.extension == ".aab"
    genrule_default_output_is_apk = genrule_default_output.extension == ".apk"

    expect(
        genrule_default_output_is_aab or genrule_default_output_is_apk,
        "apk_genrule must output a '.apk' or '.aab' file, but got {}",
        genrule_default_info,
    )

    if ctx.attrs.aab:
        if genrule_default_output_is_aab:
            output_aab_info = AndroidAabInfo(
                aab = genrule_default_output,
                manifest = input_manifest,
                materialized_artifacts = input_materialized_artifacts,
                unstripped_shared_libraries = input_unstripped_shared_libraries,
            )
            output_apk = None
        else:
            output_aab_info = None
            output_apk = genrule_default_output

        if ctx.attrs.use_derived_apk:
            expect(genrule_default_output_is_aab, "Default genrule output must end in '.aab' if use_derived_apk is True.")

            output_apk = derive_universal_apk(
                ctx = ctx,
                android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo],
                app_bundle = genrule_default_output,
                keystore = ctx.attrs.keystore[KeystoreInfo] if ctx.attrs.keystore else None,
            )
            default_providers = [
                DefaultInfo(
                    default_output = output_apk,
                    other_outputs = input_materialized_artifacts + genrule_default_info[0].other_outputs,
                    sub_targets = {
                        "aab": [
                            DefaultInfo(
                                default_outputs = [genrule_default_output],
                            )
                        ],
                        "linker_argsfiles": [input_android_aab_subtargets["linker_argsfiles"][DefaultInfo]],
                        "linker_commands": [input_android_aab_subtargets["linker_commands"][DefaultInfo]],
                        "native_build_commands": [input_android_aab_subtargets["native_build_commands"][DefaultInfo]],
                        "native_libs": [input_android_aab_subtargets["native_libs"][DefaultInfo]],
                        "unstripped_native_libraries": [input_android_aab_subtargets["unstripped_native_libraries"][DefaultInfo]],
                        "unstripped_native_libraries_files": [input_android_aab_subtargets["unstripped_native_libraries_files"][DefaultInfo]],
                        "unstripped_native_libraries_json": [input_android_aab_subtargets["unstripped_native_libraries_json"][DefaultInfo]],
                    }
                    | _forward_gatorade_phase_subtargets(input_android_aab_subtargets)
                    | _forward_optional_native_lib_subtargets(input_android_aab_subtargets),
                ),
                AndroidDerivedApkInfo(
                    apk = output_apk,
                ),
            ] + filter(lambda x: not isinstance(x, DefaultInfo), genrule_providers)
        else:
            sub_targets = {k: [v[DefaultInfo]] for k, v in genrule_default_info[0].sub_targets.items()}
            sub_targets.update(
                {
                    "linker_argsfiles": [input_android_aab_subtargets["linker_argsfiles"][DefaultInfo]],
                    "linker_commands": [input_android_aab_subtargets["linker_commands"][DefaultInfo]],
                    "native_build_commands": [input_android_aab_subtargets["native_build_commands"][DefaultInfo]],
                    "native_libs": [input_android_aab_subtargets["native_libs"][DefaultInfo]],
                    "unstripped_native_libraries": [input_android_aab_subtargets["unstripped_native_libraries"][DefaultInfo]],
                    "unstripped_native_libraries_files": [input_android_aab_subtargets["unstripped_native_libraries_files"][DefaultInfo]],
                    "unstripped_native_libraries_json": [input_android_aab_subtargets["unstripped_native_libraries_json"][DefaultInfo]],
                }
                | _forward_gatorade_phase_subtargets(input_android_aab_subtargets)
                | _forward_optional_native_lib_subtargets(input_android_aab_subtargets)
            )
            default_providers = [
                DefaultInfo(
                    default_output = genrule_default_output,
                    other_outputs = genrule_default_info[0].other_outputs,
                    sub_targets = sub_targets,
                ),
            ] + filter(lambda x: not isinstance(x, DefaultInfo), genrule_providers)

    else:
        sub_targets = {k: [v[DefaultInfo]] for k, v in genrule_default_info[0].sub_targets.items()}
        sub_targets.update(
            {
                "classpath": [input_android_apk_subtargets["classpath"][DefaultInfo]],
                "classpath_targets": [input_android_apk_subtargets["classpath_targets"][DefaultInfo]],
                "linker_argsfiles": [input_android_apk_subtargets["linker_argsfiles"][DefaultInfo]],
                "linker_commands": [input_android_apk_subtargets["linker_commands"][DefaultInfo]],
                "manifest": [input_android_apk_subtargets["manifest"][DefaultInfo]],
                "native_build_commands": [input_android_apk_subtargets["native_build_commands"][DefaultInfo]],
                "native_libs": [input_android_apk_subtargets["native_libs"][DefaultInfo]],
                "unstripped_native_libraries": [input_android_apk_subtargets["unstripped_native_libraries"][DefaultInfo]],
                "unstripped_native_libraries_files": [input_android_apk_subtargets["unstripped_native_libraries_files"][DefaultInfo]],
                "unstripped_native_libraries_json": [input_android_apk_subtargets["unstripped_native_libraries_json"][DefaultInfo]],
            }
            | _forward_gatorade_phase_subtargets(input_android_apk_subtargets)
            | _forward_optional_native_lib_subtargets(input_android_apk_subtargets)
        )
        expect(
            len(filter(lambda x: isinstance(x, TemplatePlaceholderInfo), genrule_providers)) == 0,
            "TemplatePlaceholderInfo from genrule_providers needs to be merged",
        )
        templace_placeholder_info = TemplatePlaceholderInfo(
            keyed_variables = {
                "classpath": input_android_apk_template_placeholder_info["classpath"],
                "classpath_including_targets_with_no_output": input_android_apk_template_placeholder_info["classpath_including_targets_with_no_output"],
            },
        )
        expect(genrule_default_output_is_apk, "apk_genrule output must end in '.apk'")
        output_apk = genrule_default_output
        output_aab_info = None
        default_providers = [
            DefaultInfo(
                default_output = output_apk,
                other_outputs = genrule_default_info[0].other_outputs,
                sub_targets = sub_targets,
            ),
            templace_placeholder_info,
        ] + filter(lambda x: not isinstance(x, DefaultInfo), genrule_providers)

    class_to_src_map = [ctx.attrs.apk[JavaClassToSourceMapInfo]] if (ctx.attrs.apk and JavaClassToSourceMapInfo in ctx.attrs.apk) else []

    if output_apk:
        apk_providers = [
            AndroidApkInfo(
                apk = output_apk,
                manifest = input_manifest,
                materialized_artifacts = input_materialized_artifacts,
                unstripped_shared_libraries = input_unstripped_shared_libraries,
            ),
            get_install_info(
                ctx,
                output_apk = output_apk,
                manifest = input_manifest,
                exopackage_info = None,
            ),
        ]
    else:
        apk_providers = []

    aab_providers = filter(None, [output_aab_info])
    apk_under_test_providers = filter(None, [input_android_apk_under_test_info])

    preprocessed_java_classes_providers = filter(None, [input_preprocessed_java_classes_info])

    return default_providers + apk_providers + aab_providers + apk_under_test_providers + preprocessed_java_classes_providers + class_to_src_map

apk_genrule = prelude_rule(
    name = "apk_genrule",
    impl = apk_genrule_impl,
    docs = """
        An `apk_genrule()` rule is used to post-process an APK. What
        separates an apk\\_genrule from a genrule is apk\\_genrules are known by BUCK to
        produce APKs, so commands like `buck install` or
         `buck uninstall` still work. Additionally,
         `apk_genrule()` rules can be inputs to other
         `apk_genrule()` rules.
    """,
    examples = """
        Here is an example of a couple `apk_genrule()` open up an APK, do
        some super signing, and then zipalign that APK again.

        ```
        # Building this rule will produce a file named messenger.apk.
        android_binary(
          name = 'messenger',
          manifest = 'AndroidManifest.xml',
          target = 'Google Inc.:Google APIs:16',
          keystore = '//keystores:prod',
          package_type = 'release',
          proguard_config = 'proguard.cfg',
          deps = [
            ':res',
            ':src',
          ],
        )

        apk_genrule(
          name = 'messenger_super_sign_unalign',
          apk = ':messenger',
          bash = '$(exe //java/com/facebook/sign:super_sign) --input $APK --output $OUT',
          cmd_exe = '$(exe //java/com/facebook/sign:super_sign) --input %APK% --output %OUT%',
          out = 'messenger_super_sign_unalign.apk',
        )

        apk_genrule(
          name = 'messenger_super_sign',
          apk = ':messenger_super_sign_unalign',
          bash = '$ANDROID_HOME/tools/zipalign -f 4 $APK $OUT',
          cmd_exe = '%ANDROID_HOME%\\tools\\zipalign -f 4 %APK% %OUT%',
          out = 'messenger_super_sign.apk',
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "apk": attrs.option(
                attrs.dep(),
                default = None,
                doc = """
                The input `android_binary()` rule. The path to the APK can be
                 accessed with the `$APK` shell variable. Only one of `apk` or
                 `aab` can be provided.
            """,
            ),
            "keystore": attrs.option(attrs.dep(), default = None),
        }
        | genrule_common.srcs_arg()
        | genrule_common.cmd_arg()
        | genrule_common.bash_arg()
        | genrule_common.cmd_exe_arg()
        | genrule_common.type_arg()
        | genrule_common.weight_arg()
        | {
            "out": attrs.option(
                attrs.string(),
                default = None,
                doc = """
                The name of the output file or directory. The complete path to this
                 argument is provided to the shell command through
                 the `OUT` environment variable. Only one of `out`
                 or `outs` may be present.

                For an apk_genrule the output should be a '.apk' or '.aab' file.
            """,
            ),
        }
        | genrule_common.environment_expansion_separator()
        | {
            "aab": attrs.option(
                attrs.dep(),
                default = None,
                doc = """
                The input `android_binary()` rule. The path to the AAB can be
                 accessed with the `$AAB` shell variable. Only one of `apk` or
                 `aab` can be provided.
            """,
            ),
            "cacheable": attrs.option(attrs.bool(), default = None),
            "default_outs": attrs.option(attrs.set(attrs.string(), sorted = False), default = None),
            "enable_sandbox": attrs.option(attrs.bool(), default = None),
            "is_cacheable": attrs.bool(default = False),
            "outs": attrs.option(attrs.dict(key = attrs.string(), value = attrs.set(attrs.string(), sorted = False), sorted = False), default = None),
            "remote": attrs.option(attrs.bool(), default = None),
            "type": attrs.string(default = "apk"),
            "use_derived_apk": attrs.bool(default = False),
            "_android_toolchain": toolchains_common.android(),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_java_toolchain": toolchains_common.java_for_android(),
        }
        | genrule_attributes()
        | buck.licenses_arg()
        | buck.labels_arg()
        | buck.contacts_arg()
    ),
)
