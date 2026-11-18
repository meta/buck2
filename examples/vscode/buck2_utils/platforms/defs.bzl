# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

_BUCK2_RE_CLIENT_ENABLED = read_config("buck2_re_client", "enabled", "false")

def is_remote_enabled() -> bool:
    re_enabled = _BUCK2_RE_CLIENT_ENABLED
    return re_enabled == "true"

def _execution_platforms_impl(ctx: AnalysisContext) -> list[Provider]:
    is_re_enabled = is_remote_enabled()

    name = ctx.label.raw_target()

    constraints_windows = dict()
    constraints_windows.update(ctx.attrs.os_configuration_windows[ConfigurationInfo].constraints)
    constraints_linux = dict()
    constraints_linux.update(ctx.attrs.os_configuration_linux[ConfigurationInfo].constraints)

    platforms_details = [
        ("windows", host_info().os.is_windows, constraints_windows),
        ("linux", host_info().os.is_linux, constraints_linux),
    ]

    platforms = []
    for platform_name, is_local_enabled, constraints in platforms_details:
        if is_re_enabled or is_local_enabled:
            platforms.append(
                ExecutionPlatformInfo(
                    label = name,
                    configuration = ConfigurationInfo(
                        constraints = constraints,
                        values = {},
                    ),
                    executor_config = CommandExecutorConfig(
                        local_enabled = is_local_enabled,
                        remote_enabled = is_re_enabled,
                        use_limited_hybrid = True,
                        remote_execution_properties = {
                            "OSFamily": platform_name,
                            "container-image": "docker://" + platform_name + "_build",
                        },
                        remote_execution_use_case = "buck2-default",
                        use_windows_path_separators = platform_name == "windows",
                    ),
                ),
            )

    return [
        DefaultInfo(),
        ExecutionPlatformRegistrationInfo(platforms = platforms),
    ]

execution_platforms = rule(
    impl = _execution_platforms_impl,
    attrs = {
        "os_configuration_linux": attrs.dep(
            providers = [ConfigurationInfo],
            default = "config//os:linux",
        ),
        "os_configuration_windows": attrs.dep(
            providers = [ConfigurationInfo],
            default = "config//os:windows",
        ),
    },
)
