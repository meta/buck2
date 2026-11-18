# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

_TEST_REMOTE_DEP_FILE_CACHE_ENABLED = read_config("test", "remote_dep_file_cache_enabled", "true")

ExecutorConfigInfo = provider(fields = ["config"])

def _platform(ctx):
    # We need to introduce a constraint to ensure our different execution
    # platforms are distinct. This is because exec_compatible_with selects a
    # ConfigurationInfo (which provides a config), not a ExecutionPlatformInfo
    # (instead it matches on it).
    configuration = ConfigurationInfo(
        constraints = {
            ctx.attrs.setting.label.raw_target(): ConstraintValueInfo(
                setting = ctx.attrs.setting[ConstraintSettingInfo],
                label = ctx.label.raw_target(),
            ),
        },
        values = {},
    )

    platform = ExecutionPlatformInfo(
        label = ctx.label.raw_target(),
        configuration = configuration,
        executor_config = CommandExecutorConfig(
            local_enabled = ctx.attrs.local_enabled,
            remote_enabled = ctx.attrs.remote_enabled,
            remote_execution_properties = {
                "platform": "linux-remote-execution",
            },
            remote_execution_max_input_files_mebibytes = 1,
            use_limited_hybrid = ctx.attrs.use_limited_hybrid,
            allow_limited_hybrid_fallbacks = ctx.attrs.allow_hybrid_fallbacks_on_failure,
            allow_hybrid_fallbacks_on_failure = ctx.attrs.allow_hybrid_fallbacks_on_failure,
            remote_execution_use_case = "buck2-testing",
            allow_cache_uploads = ctx.attrs.allow_cache_uploads,
            remote_dep_file_cache_enabled = ctx.attrs.remote_dep_file_cache_enabled and _TEST_REMOTE_DEP_FILE_CACHE_ENABLED == "true",
            max_cache_upload_mebibytes = 1,
        ),
    )

    return [
        DefaultInfo(),
        platform,
        configuration,
    ]

platform = rule(
    impl = _platform,
    attrs = {
        "allow_cache_uploads": attrs.bool(default = False),
        "allow_hybrid_fallbacks_on_failure": attrs.bool(default = False),
        "local_enabled": attrs.bool(default = True),
        "remote_dep_file_cache_enabled": attrs.bool(default = False),
        "remote_enabled": attrs.bool(default = True),
        "setting": attrs.configuration_label(),
        "use_limited_hybrid": attrs.bool(default = True),
    },
)

def _platforms(ctx):
    return [
        DefaultInfo(),
        ExecutionPlatformRegistrationInfo(
            platforms = [x[ExecutionPlatformInfo] for x in ctx.attrs.platforms],
        ),
    ]

platforms = rule(
    impl = _platforms,
    attrs = {
        "platforms": attrs.list(attrs.dep(providers = [ExecutionPlatformInfo])),
    },
)

def _target_platform(ctx):
    return [
        DefaultInfo(),
        PlatformInfo(
            label = str(ctx.label.raw_target()),
            configuration = ConfigurationInfo(constraints = {}, values = {}),
        ),
    ]

target_platform = rule(
    impl = _target_platform,
    attrs = {},
)

def _config_setting(ctx):
    return [DefaultInfo(), ConstraintSettingInfo(label = ctx.label.raw_target())]

config_setting = rule(
    impl = _config_setting,
    attrs = {},
)
