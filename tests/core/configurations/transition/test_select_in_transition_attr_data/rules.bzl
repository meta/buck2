# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

def _impl_tr(platform, refs, attrs):
    constraints = {s: v for (s, v) in platform.configuration.constraints.items() if s != refs.os[ConstraintSettingInfo].label}

    watchos = refs.watchos[ConstraintValueInfo]
    iphoneos = refs.iphoneos[ConstraintValueInfo]
    if attrs.device == "watch":
        constraints[watchos.setting.label] = watchos
    elif attrs.device == "phone":
        constraints[iphoneos.setting.label] = iphoneos
    else:
        fail()
    new_cfg = ConfigurationInfo(
        constraints = constraints,
        values = platform.configuration.values,
    )
    return PlatformInfo(
        label = "<transitioned-from-" + attrs.device + ">",
        configuration = new_cfg,
    )

iphone_or_watch_transition = transition(
    impl = _impl_tr,
    refs = {
        "iphoneos": "root//:iphoneos",
        "os": "root//:os",
        "watchos": "root//:watchos",
    },
    attrs = [
        "device",
        "extra",
    ],
)

def _impl(_ctx):
    return [DefaultInfo()]

test_rule = rule(impl = _impl, attrs = {"device": attrs.string(), "extra": attrs.string(default = "")}, cfg = iphone_or_watch_transition)

test_rule_with_transition_dep = rule(
    impl = _impl,
    attrs = {"attr_with_transition": attrs.transition_dep(cfg = iphone_or_watch_transition), "device": attrs.string(), "extra": attrs.string(default = "")},
)

any_rule = rule(impl = _impl, attrs = {"another_device": attrs.string()})
