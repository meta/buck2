# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

CLASSPATH_KEY = "classpath_including_targets_with_no_output"

def _target_impl(ctx):
    out = ctx.actions.write("out", ctx.attrs.arg or "", allow_args = True, has_content_based_path = False)[0]
    classpaths = [out]
    if ctx.attrs.dep:
        classpaths.append(ctx.attrs.dep[TemplatePlaceholderInfo].keyed_variables[CLASSPATH_KEY])
    return [
        DefaultInfo(default_output = out),
        RunInfo(),
        TemplatePlaceholderInfo(
            keyed_variables = {
                CLASSPATH_KEY: cmd_args(classpaths),
            }
        ),
    ]

target = rule(
    impl = _target_impl,
    attrs = {
        "arg": attrs.option(attrs.arg(), default = None),
        "dep": attrs.option(attrs.dep(), default = None),
    },
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
        arg = ("$(query_targets classpath(:dep))"),
        default_target_platform = ":target_platform",
    )

    target(name = "dep", dep = ":select-dep")

    target(
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
