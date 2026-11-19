# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# This is copy-paste from `prelude/configurations/util.bzl`

def _configuration_info(constraints, values, root_values = None):
    return ConfigurationInfo(
        constraints = constraints,
        values = values,
        root_values = root_values or {},
    )

def _configuration_info_union(infos):
    if len(infos) == 0:
        return _configuration_info(
            constraints = {},
            values = {},
        )
    if len(infos) == 1:
        return infos[0]
    constraints = {k: v for info in infos for (k, v) in info.constraints.items()}
    values = {k: v for info in infos for (k, v) in info.values.items()}
    root_values = {k: v for info in infos for (k, v) in info.root_values.items()}
    return _configuration_info(
        constraints = constraints,
        values = values,
        root_values = root_values,
    )

def _constraint_values_to_configuration(values):
    return _configuration_info(
        constraints = {info[ConstraintValueInfo].setting.label: info[ConstraintValueInfo] for info in values},
        values = {},
    )

# This is copy-paste from `prelude/configurations/rules.bzl`

def _constraint_setting_impl(ctx):
    return [DefaultInfo(), ConstraintSettingInfo(label = ctx.label.raw_target())]

constraint_setting = rule(
    impl = _constraint_setting_impl,
    is_configuration_rule = True,
    attrs = {},
)

def _constraint_value_impl(ctx):
    constraint_value = ConstraintValueInfo(
        setting = ctx.attrs.constraint_setting[ConstraintSettingInfo],
        label = ctx.label.raw_target(),
    )
    return [
        DefaultInfo(),
        constraint_value,
        # Provide `ConfigurationInfo` from `constraint_value` so it could be used as select key.
        _configuration_info(
            constraints = {
                constraint_value.setting.label: constraint_value,
            },
            values = {},
        ),
    ]

constraint_value = rule(
    impl = _constraint_value_impl,
    is_configuration_rule = True,
    attrs = {
        "constraint_setting": attrs.dep(providers = [ConstraintSettingInfo]),
    },
)

def _platform_impl(ctx):
    subinfos = [dep[PlatformInfo].configuration for dep in ctx.attrs.deps] + [_constraint_values_to_configuration(ctx.attrs.constraint_values)]
    return [
        DefaultInfo(),
        PlatformInfo(
            label = str(ctx.label.raw_target()),
            configuration = _configuration_info_union(subinfos),
        ),
    ]

platform = rule(
    impl = _platform_impl,
    is_configuration_rule = True,
    attrs = {
        "constraint_values": attrs.list(attrs.dep(providers = [ConfigurationInfo]), default = []),
        "deps": attrs.list(attrs.dep(providers = [PlatformInfo]), default = []),
    },
)

def _config_setting_impl(ctx):
    subinfos = [_constraint_values_to_configuration(ctx.attrs.constraint_values)]
    subinfos.append(
        _configuration_info(
            constraints = {},
            values = ctx.attrs.values,
            root_values = ctx.attrs.root_values,
        )
    )
    return [DefaultInfo(), _configuration_info_union(subinfos)]

config_setting = rule(
    impl = _config_setting_impl,
    is_configuration_rule = True,
    attrs = {
        "constraint_values": attrs.list(attrs.dep(providers = [ConstraintValueInfo]), default = []),
        "root_values": attrs.dict(attrs.string(), attrs.string(), default = {}),
        "values": attrs.dict(attrs.string(), attrs.string(), default = {}),
    },
)

def _execution_platform(ctx):
    return [
        DefaultInfo(),
        ExecutionPlatformInfo(
            label = ctx.label.raw_target(),
            configuration = ctx.attrs.platform[PlatformInfo].configuration,
            executor_config = CommandExecutorConfig(
                local_enabled = ctx.attrs.local_enabled,
                remote_enabled = ctx.attrs.remote_enabled,
                remote_execution_properties = {
                    "platform": "linux-remote-execution",
                },
                remote_execution_max_input_files_mebibytes = 1,
                use_limited_hybrid = True,
                allow_limited_hybrid_fallbacks = False,
                allow_hybrid_fallbacks_on_failure = False,
                remote_execution_use_case = "buck2-testing",
                remote_cache_enabled = ctx.attrs.remote_cache_enabled,
                allow_cache_uploads = ctx.attrs.allow_cache_uploads,
                max_cache_upload_mebibytes = 1,
            ),
        ),
    ]

execution_platform = rule(
    impl = _execution_platform,
    attrs = {
        "allow_cache_uploads": attrs.bool(default = False),
        "local_enabled": attrs.bool(default = True),
        "platform": attrs.dep(providers = [PlatformInfo]),
        "remote_cache_enabled": attrs.option(attrs.bool(), default = None),
        "remote_enabled": attrs.bool(default = False),
    },
)
