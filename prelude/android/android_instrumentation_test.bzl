# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@prelude//:prelude_rule.bzl", "prelude_rule")
load("@prelude//android:android_providers.bzl", "AndroidApkExopackageInfo", "AndroidApkInfo", "AndroidInstrumentationApkInfo")
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//decls:test_common.bzl", "test_common")
load("@prelude//decls:toolchains_common.bzl", "toolchains_common")
load("@prelude//java:class_to_srcs.bzl", "JavaClassToSourceMapInfo")
load("@prelude//java:java_providers.bzl", "JavaPackagingInfo", "get_all_java_packaging_deps_tset")
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")
load("@prelude//java/utils:java_more_utils.bzl", "get_path_separator_for_exec_os")
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
    "create_shlib_symlink_tree",
    "merge_shared_libraries",
    "traverse_shared_library_info",
)
load("@prelude//target_stats:target_stats.bzl", "CycleMode", "target_stats_providers_and_subtargets")
load("@prelude//target_stats:target_stats_config.bzl", "TARGET_STATS_ENABLED")
load("@prelude//test:inject_test_run_info.bzl", "inject_test_run_info")
load("@prelude//tests:test_listing.bzl", "TestListingInfo")
load("@prelude//utils:argfile.bzl", "at_argfile")
load("@prelude//utils:clear_platform.bzl", "clear_platform_transition")
load("@prelude//utils:expect.bzl", "expect")
load(":attrs.bzl", "META_INTERNAL_EXTRA_PARAMS", "RE_CAPS", "RE_USE_CASE")

ANDROID_EMULATOR_ABI_LABEL_PREFIX = "tpx-re-config::"
DEFAULT_ANDROID_SUBPLATFORM = "android-30"
DEFAULT_ANDROID_PLATFORM = "android-emulator"
DEFAULT_ANDROID_INSTRUMENTATION_TESTS_USE_CASE = "instrumentation-tests"
RIOT_USE_CASES = [
    "foundation-diff",
    "horizon-experiences-diff",
    "horizon-os-codemod-diff",
    "horizon-os-diff",
    "horizon-os-human-lease",
    "horizon-os-other",
    "vr-diff",
    "wearables-diff",
    "wearables-human-lease",
    "wearables-mqb",
    "wearables-other",
]
SUPPORTED_POOLS = ["EUREKA_POOL", "HOLLYWOOD_POOL", "STAGE_DELPHI_POOL", "PANTHER_POOL", "SEACLIFF_POOL"]
SUPPORTED_PLATFORMS = ["riot", "android-emulator", "rl-emulator"]
SUPPORTED_USE_CASES = RIOT_USE_CASES + [DEFAULT_ANDROID_INSTRUMENTATION_TESTS_USE_CASE]

_AOSP_REPLAY_OBI_BUILD = read_root_config("android_instrumentation", "aosp_replay_obi_build", None)
_AOSP_REPLAY_OBI_BUILD_LABEL_PREFIX = "aosp_replay_obi_build="

def android_instrumentation_test_impl(ctx: AnalysisContext):
    android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo]

    cmd = [ctx.attrs._java_test_toolchain[JavaToolchainInfo].java_for_tests]

    classpath = android_toolchain.instrumentation_test_runner_classpath

    classpath_args = cmd_args()
    classpath_args.add("-classpath")
    env = ctx.attrs.env or {}
    extra_classpath = []
    if ctx.attrs.instrumentation_test_listener != None:
        extra_classpath.extend([
            get_all_java_packaging_deps_tset(ctx, java_packaging_infos = [ctx.attrs.instrumentation_test_listener[JavaPackagingInfo]]).project_as_args(
                "full_jar_args", ordering = "bfs"
            ),
        ])

        shared_library_info = merge_shared_libraries(
            ctx.actions,
            deps = [ctx.attrs.instrumentation_test_listener[SharedLibraryInfo]],
        )

        cxx_library_symlink_tree = create_shlib_symlink_tree(
            actions = ctx.actions,
            out = "cxx_library_symlink_tree",
            shared_libs = traverse_shared_library_info(shared_library_info, transformation_provider = None),
        )

        env["BUCK_LD_SYMLINK_TREE"] = cxx_library_symlink_tree
    classpath_args.add(cmd_args(extra_classpath + classpath, delimiter = get_path_separator_for_exec_os(ctx)))
    cmd.append(at_argfile(actions = ctx.actions, name = "classpath_args_file", args = classpath_args))

    cmd.append(android_toolchain.instrumentation_test_runner_main_class)

    apk_info = ctx.attrs.apk.get(AndroidApkInfo)
    expect(apk_info != None, "Provided APK must have AndroidApkInfo!")

    instrumentation_apk_info = ctx.attrs.apk.get(AndroidInstrumentationApkInfo)
    # A self-instrumenting apk already bundles the app-under-test's contents, and its manifest's
    # targetPackage is the self apk itself rather than the un-merged app-under-test apk's package,
    # so there is no separate app-under-test to install.
    if instrumentation_apk_info != None and not instrumentation_apk_info.is_self_instrumenting:
        cmd.extend(["--apk-under-test-path", instrumentation_apk_info.apk_under_test])
    if ctx.attrs.is_self_instrumenting:
        cmd.extend(["--is-self-instrumenting"])
    extra_instrumentation_args = ctx.attrs.extra_instrumentation_args
    if extra_instrumentation_args:
        for arg_name, arg_value in extra_instrumentation_args.items():
            cmd.extend(
                [
                    "--extra-instrumentation-argument",
                    cmd_args([arg_name, arg_value], delimiter = "="),
                ],
            )

    target_package_file = ctx.actions.declare_output("target_package_file", has_content_based_path = False)
    package_file = ctx.actions.declare_output("package_file", has_content_based_path = False)
    test_runner_file = ctx.actions.declare_output("test_runner_file", has_content_based_path = False)
    manifest_utils_cmd = cmd_args(ctx.attrs._android_toolchain[AndroidToolchainInfo].manifest_utils[RunInfo])
    manifest_utils_cmd.add([
        "--manifest-path",
        apk_info.manifest,
        "--package-output",
        package_file.as_output(),
        "--target-package-output",
        target_package_file.as_output(),
        "--instrumentation-test-runner-output",
        test_runner_file.as_output(),
    ])
    ctx.actions.run(manifest_utils_cmd, category = "get_manifest_info")
    cmd.extend(
        [
            "--test-package-name",
            cmd_args(package_file, format = "@{}"),
            "--target-package-name",
            cmd_args(target_package_file, format = "@{}"),
            "--test-runner",
            cmd_args(test_runner_file, format = "@{}"),
        ],
    )

    if ctx.attrs.instrumentation_test_listener_class != None:
        cmd.extend(["--extra-instrumentation-test-listener", ctx.attrs.instrumentation_test_listener_class])

    if ctx.attrs.clear_package_data:
        cmd.append("--clear-package-data")

    if ctx.attrs.disable_animations:
        cmd.append("--disable-animations")

    if ctx.attrs.collect_tombstones:
        cmd.append("--collect-tombstones")
    if ctx.attrs.record_video:
        cmd.append("--record-video")
    if android_toolchain.collect_perfetto:
        cmd.append("--collect-perfetto")
    if ctx.attrs.log_extractors:
        for arg_name, arg_value in ctx.attrs.log_extractors.items():
            cmd.extend(
                [
                    "--log-extractor",
                    cmd_args([arg_name, arg_value], delimiter = "="),
                ],
            )

    cmd.extend(
        [
            "--adb-executable-path",
            "required_but_unused",
            "--instrumentation-apk-path",
            apk_info.apk,
        ],
    )

    # Exopackage secondary dexes live out-of-band, not in base.apk. Pass the build's secondary-dex
    # dir so the runner pushes them to /data/local/tmp/exopackage/<pkg>/secondary-dex before
    # Application init; otherwise classes in those dexes (e.g. com.facebook.R$style) hit NoClassDefFoundError on RE.
    apk_exopackage_info = ctx.attrs.apk.get(AndroidApkExopackageInfo)
    if apk_exopackage_info != None:
        cmd.extend([
            "--exopackage-secondary-dex-local-dir",
            apk_exopackage_info.secondary_dex_directory,
        ])

    listing_info = ctx.attrs._android_toolchain[TestListingInfo]

    list_tests = listing_info.list_tests
    if list_tests != None and "tpx:supports_static_listing=true" in ctx.attrs.labels and "tpx:supports_static_listing=false" not in ctx.attrs.labels:
        list_tests_command = cmd_args([
            list_tests[RunInfo],
            "list-tests",
            "--sources-file",
            ctx.actions.write("source_files.txt", ctx.attrs._test_srcs, with_inputs = True, has_content_based_path = False),
        ])
        env["TPX_LIST_TESTS_COMMAND"] = list_tests_command

    labels = list(ctx.attrs.labels)
    if "tpx:supports-test-result-output-spec" not in labels:
        labels.append("tpx:supports-test-result-output-spec")
    if _AOSP_REPLAY_OBI_BUILD:
        labels = [label for label in labels if not label.startswith(_AOSP_REPLAY_OBI_BUILD_LABEL_PREFIX)]
        labels.append(_AOSP_REPLAY_OBI_BUILD_LABEL_PREFIX + _AOSP_REPLAY_OBI_BUILD)

    test_info = ExternalRunnerTestInfo(
        type = "android_instrumentation",
        command = cmd,
        env = env,
        labels = labels,
        contacts = ctx.attrs.contacts,
        run_from_project_root = True,
        use_project_relative_paths = True,
        executor_overrides = _compute_executor_overrides(ctx, android_toolchain.instrumentation_test_can_run_locally),
        local_resources = {
            "android_emulator": None if ctx.attrs._android_emulators == None else ctx.attrs._android_emulators.label,
        },
        required_local_resources = [RequiredTestLocalResource("android_emulator", listing = True, execution = True)],
    )

    classmap_source_info = [ctx.attrs.apk[JavaClassToSourceMapInfo]] if JavaClassToSourceMapInfo in ctx.attrs.apk else []

    test_info, run_info = inject_test_run_info(ctx, test_info)

    target_stats_providers = []
    target_stats_subtargets = {}
    if TARGET_STATS_ENABLED:
        target_stats_tools = android_toolchain.target_stats_tools
        if target_stats_tools != None:
            target_stats_deps = [ctx.attrs.apk]
            if ctx.attrs.instrumentation_test_listener != None:
                target_stats_deps.append(ctx.attrs.instrumentation_test_listener)
            target_stats_providers, target_stats_subtargets = target_stats_providers_and_subtargets(
                ctx,
                tools = target_stats_tools,
                srcs = {src.short_path: src for src in ctx.attrs._test_srcs},
                deps = target_stats_deps,
                cycle_mode = CycleMode("package"),
                module_name = ctx.label.name,
            )

    # We append additional args so that "buck2 run" will work with sane defaults
    run_info.args.add(cmd_args(["--auto-run-on-connected-device", "--output", ".", "--adb-executable-path", "adb"]))
    return (
        [
            test_info,
            run_info,
            DefaultInfo(sub_targets = target_stats_subtargets),
        ]
        + classmap_source_info
        + target_stats_providers
    )

def _compute_executor_overrides(ctx: AnalysisContext, instrumentation_test_can_run_locally: bool) -> dict[str, CommandExecutorConfig]:
    remote_execution_properties = {
        "platform": _compute_emulator_platform(ctx.attrs.labels or []),
        "subplatform": _compute_emulator_subplatform(ctx.attrs.labels or []),
    }

    re_emulator_abi = _compute_emulator_abi(ctx.attrs.labels or [])
    if re_emulator_abi != None:
        remote_execution_properties["abi"] = re_emulator_abi

    default_executor_override = CommandExecutorConfig(
        local_enabled = instrumentation_test_can_run_locally,
        remote_enabled = True,
        remote_execution_properties = remote_execution_properties,
        remote_execution_use_case = _compute_re_use_case(ctx.attrs.labels or []),
        meta_internal_extra_params = _meta_internal_extra_params_for(remote_execution_properties, None),
    )
    dynamic_listing_executor_override = default_executor_override
    test_execution_executor_override = default_executor_override

    if ctx.attrs.re_caps and ctx.attrs.re_use_case:
        if "dynamic-listing" in ctx.attrs.re_caps and "dynamic-listing" in ctx.attrs.re_use_case:
            _validate_executor_override_re_config(ctx.attrs.re_caps["dynamic-listing"], ctx.attrs.re_use_case["dynamic-listing"])
            dynamic_listing_executor_override = CommandExecutorConfig(
                local_enabled = instrumentation_test_can_run_locally,
                remote_enabled = True,
                remote_execution_properties = ctx.attrs.re_caps["dynamic-listing"],
                remote_execution_use_case = ctx.attrs.re_use_case["dynamic-listing"],
                meta_internal_extra_params = _meta_internal_extra_params_for(ctx.attrs.re_caps["dynamic-listing"], ctx.attrs.meta_internal_extra_params),
            )
        if "test-execution" in ctx.attrs.re_caps and "test-execution" in ctx.attrs.re_use_case:
            _validate_executor_override_re_config(ctx.attrs.re_caps["test-execution"], ctx.attrs.re_use_case["test-execution"])
            test_execution_executor_override = CommandExecutorConfig(
                local_enabled = instrumentation_test_can_run_locally,
                remote_enabled = True,
                remote_execution_properties = ctx.attrs.re_caps["test-execution"],
                remote_execution_use_case = ctx.attrs.re_use_case["test-execution"],
                meta_internal_extra_params = _meta_internal_extra_params_for(ctx.attrs.re_caps["test-execution"], ctx.attrs.meta_internal_extra_params),
            )

    return {
        "android-emulator": default_executor_override,
        "dynamic-listing": dynamic_listing_executor_override,
        "static-listing": CommandExecutorConfig(
            ## This was set to True as some point and it was causing listing to happen locally,
            ## which is one of the contributing factors to S504068.
            local_enabled = instrumentation_test_can_run_locally,
            remote_enabled = True,
            remote_execution_properties = {
                "platform": "linux-remote-execution",
            },
            remote_execution_use_case = "buck2-default",
        ),
        "test-execution": test_execution_executor_override,
    }

def _compute_emulator_abi(labels: list[str]):
    emulator_abi_labels = [label for label in labels if label.startswith(ANDROID_EMULATOR_ABI_LABEL_PREFIX)]
    expect(
        len(emulator_abi_labels) <= 1,
        "multiple '{}' labels were found:[{}], there must be only one!".format(ANDROID_EMULATOR_ABI_LABEL_PREFIX, ", ".join(emulator_abi_labels)),
    )
    if len(emulator_abi_labels) == 0:
        return None
    else:  # len(emulator_abi_labels) == 1:
        return emulator_abi_labels[0].replace(ANDROID_EMULATOR_ABI_LABEL_PREFIX, "")

# replicating the logic in https://fburl.com/code/1fqowxu4 to match buck1's behavior
def _compute_emulator_subplatform(labels: list[str]) -> str:
    emulator_subplatform_labels = [label for label in labels if label.startswith("re_emulator_")]
    expect(
        len(emulator_subplatform_labels) <= 1,
        "multiple 're_emulator_' labels were found:[{}], there must be only one!".format(", ".join(emulator_subplatform_labels)),
    )
    if len(emulator_subplatform_labels) == 0:
        return DEFAULT_ANDROID_SUBPLATFORM
    else:  # len(emulator_subplatform_labels) == 1:
        return emulator_subplatform_labels[0].replace("re_emulator_", "")

def _compute_emulator_platform(labels: list[str]) -> str:
    emulator_platform_labels = [label for label in labels if label.startswith("re_platform_")]
    expect(
        len(emulator_platform_labels) <= 1,
        "multiple 're_platform_' labels were found:[{}], there must be only one!".format(", ".join(emulator_platform_labels)),
    )
    if len(emulator_platform_labels) == 0:
        subplatform = _compute_emulator_subplatform(labels)
        if "aarch64" in subplatform:
            return "android-emulator-aarch64"
        if subplatform == "android-33-google-arm64":
            return "android-emulator-mac"
        return DEFAULT_ANDROID_PLATFORM
    else:  # len(emulator_platform_labels) == 1:
        return emulator_platform_labels[0].replace("re_platform_", "")

def _compute_re_use_case(labels: list[str]) -> str:
    re_use_case_labels = [label for label in labels if label.startswith("re_opts_use_case=")]
    expect(len(re_use_case_labels) <= 1, "multiple 're_opts_use_case' labels were found:[{}], there must be only one!".format(", ".join(re_use_case_labels)))
    if len(re_use_case_labels) == 0:
        return DEFAULT_ANDROID_INSTRUMENTATION_TESTS_USE_CASE
    else:  # len(re_use_case_labels) == 1:
        return re_use_case_labels[0].replace("re_opts_use_case=", "")

# A `riot` run holds a lab device until it ends, so have RE stop it once buck2 disconnects.
def _meta_internal_extra_params_for(re_caps: dict[str, str], meta_internal_extra_params: dict[str, typing.Any] | None) -> dict[str, typing.Any] | None:
    if re_caps.get("platform") != "riot":
        return meta_internal_extra_params
    return (meta_internal_extra_params or {}) | {"cancel_on_client_disconnect": True}

def _validate_executor_override_re_config(re_caps: dict[str, str], re_use_case: str):
    expect(
        re_use_case in SUPPORTED_USE_CASES,
        "Unexpected {} use case found, value is expected to be on of the following: {}",
        re_use_case,
        ", ".join(SUPPORTED_USE_CASES),
    )
    if "pool" in re_caps:
        expect(
            re_caps["pool"] in SUPPORTED_POOLS,
            "Unexpected {} pool found, value is expected to be on of the following: {}",
            re_caps["pool"],
            ", ".join(SUPPORTED_POOLS),
        )
    if "platform" in re_caps:
        expect(
            re_caps["platform"] in SUPPORTED_PLATFORMS,
            "Unexpected {} platform found, value is expected to be on of the following: {}",
            re_caps["platform"],
            ", ".join(SUPPORTED_PLATFORMS),
        )

android_instrumentation_test = prelude_rule(
    name = "android_instrumentation_test",
    impl = android_instrumentation_test_impl,
    docs = """
        An `android_instrumentation_test()` rule is used to define
         apks that should be used to run Android instrumentation tests.
    """,
    examples = """
        Here is an example of an `android_instrumentation_test()`
         rule that tests an `android_binary()`.

        ```
        android_binary(
          name = 'messenger',
          manifest = 'AndroidManifest.xml',
          keystore = '//keystores:prod',
          package_type = 'release',
          proguard_config = 'proguard.cfg',
          deps = [
            ...
          ],
        )

        android_instrumentation_apk(
          name = 'messenger_test',
          manifest = 'AndroidInstrumentationManifest.xml',
          apk = ':messenger',
          deps = [
            ...
          ],
        )

        android_instrumentation_test(
          name = 'messenger_instrumentation_test',
          apk = ':messenger_test',
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        buck.inject_test_env_arg()
        | {
            "apk": attrs.dep(
                doc = """
                The APK containing the tests. Can be an `android_binary()`,
                 an `apk_genrule()` or an `android_instrumentation_apk()`.
            """,
            ),
        }
        | buck.test_label_arg()
        | buck.test_rule_timeout_ms()
        | buck.licenses_arg()
        | buck.contacts_arg()
        | {
            "clear_package_data": attrs.bool(
                default = False,
                doc = """
                Runs `pm clear` on the app and test packages before the test run if set to True.
            """,
            ),
            "collect_tombstones": attrs.bool(
                default = False,
                doc = """
                Checks whether the test generated any tombstones, and downloads them from the emulator if true.
            """,
            ),
            "disable_animations": attrs.bool(
                default = False,
                doc = """
                Disables animations on the emulator if set to True.
            """,
            ),
            "env": attrs.dict(key = attrs.string(), value = attrs.arg(), sorted = False, default = {}),
            "extra_instrumentation_args": attrs.option(attrs.dict(key = attrs.string(), value = attrs.arg()), default = None),
            "instrumentation_test_listener": attrs.option(attrs.exec_dep(), default = None),
            "instrumentation_test_listener_class": attrs.option(attrs.string(), default = None),
            "is_self_instrumenting": attrs.bool(default = False),
            "log_extractors": attrs.dict(key = attrs.string(), value = attrs.string(), sorted = False, default = {}),
            "meta_internal_extra_params": META_INTERNAL_EXTRA_PARAMS,
            "re_caps": RE_CAPS,
            "re_use_case": RE_USE_CASE,
            "record_video": attrs.bool(default = False, doc = "Record video of test run and collect it as TRA"),
            "_android_emulators": attrs.option(
                attrs.transition_dep(cfg = clear_platform_transition, providers = [LocalResourceInfo]),
                default = None,
                doc = """
                If provided, local resource of "android_emulators" type will be required to run this test locally and this target will be used to manage it. If omitted, local resource of "android_emulators" type will be ignored even if requested by the test runner.
            """,
            ),
            "_android_toolchain": toolchains_common.android(),
            "_exec_os_type": buck.exec_os_type_arg(),
            "_java_test_toolchain": toolchains_common.java_for_host_test(),
            "_java_toolchain": toolchains_common.java_for_android(),
            "_test_srcs": attrs.list(attrs.source(allow_directory = True), default = []),
        }
        | test_common.attributes()
    ),
)
