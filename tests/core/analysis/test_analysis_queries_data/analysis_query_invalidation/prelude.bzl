# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

target = rule(
    impl = lambda ctx: [
        DefaultInfo(default_output = ctx.actions.write("out", ctx.attrs.arg or "", allow_args = True, has_content_based_path = False)[0]),
        RunInfo(),
    ],
    attrs = {"arg": attrs.option(attrs.arg(), default = None)},
)

proxy_target = rule(
    impl = lambda ctx: ctx.attrs.dep.providers,
    attrs = {"dep": attrs.dep()},
)

def _platform(ctx):
    # Configuration that reads from buckconfig
    configuration = ConfigurationInfo(
        constraints = {},
        values = ctx.attrs.values,
    )

    return [
        DefaultInfo(),
        configuration,
    ]

config = rule(
    impl = _platform,
    attrs = {
        "setting": attrs.configuration_label(),
        "values": attrs.dict(
            key = attrs.string(),
            value = attrs.string(),
            sorted = False,
            default = {},
        ),
    },
)

def _config_setting(ctx):
    return [DefaultInfo(), ConstraintSettingInfo(label = ctx.label.raw_target())]

config_setting = rule(
    impl = _config_setting,
    attrs = {},
)

def _target_platform_impl(ctx):
    return [
        DefaultInfo(),
        PlatformInfo(
            label = str(ctx.label.raw_target()),
            configuration = ConfigurationInfo(constraints = {}, values = {}),
        ),
    ]

target_platform = rule(impl = _target_platform_impl, attrs = {})

def defs():
    target(
        name = "root",
        arg = ("$(query_targets deps(:dep))"),
        default_target_platform = ":target_platform",
    )

    proxy_target(name = "dep", dep = ":select-dep")

    proxy_target(
        name = "select-dep",
        dep = select(
            {
                ":linux": ":linux-select-dep",
                ":macos": ":macos-select-dep",
            },
        ),
    )

    target(
        name = "macos-select-dep",
        target_compatible_with = [":macos"],
    )

    target(
        name = "linux-select-dep",
        target_compatible_with = [":linux"],
    )

    config_setting(name = "setting")
    config(name = "linux", setting = ":setting", values = {"test.configuration": "linux"})
    config(name = "macos", setting = ":setting", values = {"test.configuration": "macos"})

    target_platform(name = "target_platform")
