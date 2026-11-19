# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# @oss-disable[end= ]: load("@prelude//:is_full_meta_repo.bzl", "is_full_meta_repo")
load("@prelude//go:coverage.bzl", "GoCoverageMode")
# @oss-disable[end= ]: load("@prelude//go/meta_only:transitions.bzl", "IMPLICIT_SANITIZER_REFS", "disable_implicit_sanitizers")
load(":tags_helper.bzl", "selects_for_tags", "tag_to_constraint_value")

def _cgo_enabled_transition(platform, refs, attrs):
    # If attribute is None, do nothing
    if attrs.cgo_enabled == None:
        return platform

    # Override configuration with attribute value
    configuration = platform.configuration
    if attrs.cgo_enabled == True:
        cgo_enabled_value = refs.cgo_enabled_true[ConstraintValueInfo]
    else:
        cgo_enabled_value = refs.cgo_enabled_false[ConstraintValueInfo]

    configuration.insert(cgo_enabled_value)

    return PlatformInfo(
        label = platform.label,
        configuration = configuration,
    )

def _coverage_mode_transition(platform, refs, attrs):
    # If attribute is None, do nothing
    if attrs.coverage_mode == None:
        return platform

    # Override configuration with attribute value
    configuration = platform.configuration
    if attrs.coverage_mode == "set":
        coverage_mode_value = refs.coverage_mode_set[ConstraintValueInfo]
    elif attrs.coverage_mode == "count":
        coverage_mode_value = refs.coverage_mode_count[ConstraintValueInfo]
    elif attrs.coverage_mode == "atomic":
        coverage_mode_value = refs.coverage_mode_atomic[ConstraintValueInfo]
    else:
        fail("`coverage_mode` can be either: 'set', 'count', 'atomic' or None, got: {}".format(attrs.coverage_mode))

    configuration.insert(coverage_mode_value)

    return PlatformInfo(
        label = platform.label,
        configuration = configuration,
    )

def _tags_transition(platform, refs, attrs):
    # If no build tags, do nothing
    if not attrs.build_tags:
        return platform

    # Override configuration with attribute values
    configuration = platform.configuration
    for build_tag in attrs.build_tags:
        ref_name = "tag_{}__set".format(build_tag)
        if not hasattr(refs, ref_name):
            fail("Add build_tag '{}' to .buckconfig attribute `go.allowed_build_tags` to allow using it".format(build_tag))

        tag_value = getattr(refs, ref_name)[ConstraintValueInfo]
        configuration.insert(tag_value)

    return PlatformInfo(
        label = platform.label,
        configuration = configuration,
    )

def _force_mingw_on_windows(platform, refs, _):
    constraints = platform.configuration.constraints

    abi_gnu_value = refs.abi_gnu[ConstraintValueInfo]
    if abi_gnu_value.setting.label in constraints and constraints[abi_gnu_value.setting.label] == abi_gnu_value:
        # Already MinGW/GNU, do nothing
        return platform

    os_windows_value = refs.os_windows[ConstraintValueInfo]
    if os_windows_value.setting.label in constraints and constraints[os_windows_value.setting.label] != os_windows_value:
        # Non-Windows, do nothing
        return platform

    constraints[abi_gnu_value.setting.label] = abi_gnu_value

    new_cfg = ConfigurationInfo(
        constraints = constraints,
        values = platform.configuration.values,
    )

    return PlatformInfo(
        label = platform.label,
        configuration = new_cfg,
    )

def _chain_transitions(transitions):
    def tr(platform, refs, attrs):
        for t in transitions:
            platform = t(platform, refs, attrs)
        return platform

    return tr

_all_level_transitions = [_force_mingw_on_windows]
_top_level_transitions = [_cgo_enabled_transition, _tags_transition] + _all_level_transitions
_executable_transitions = list(_top_level_transitions)
# @oss-disable[end= ]: _executable_transitions.extend([disable_implicit_sanitizers] if is_full_meta_repo() else [])

_all_level_refs = {
    "abi_gnu": "prelude//abi/constraints:gnu",
    "os_windows": "config//os/constraints:windows",
}

_top_level_refs = (
    {
        "cgo_enabled_false": "prelude//go/constraints:cgo_enabled[false]",
        "cgo_enabled_true": "prelude//go/constraints:cgo_enabled[true]",
    }
    | {"tag_{}__set".format(tag): constraint_value for tag, constraint_value in tag_to_constraint_value().items()}
    | _all_level_refs
)
_executable_refs = dict(_top_level_refs)
# @oss-disable[end= ]: _executable_refs.update(IMPLICIT_SANITIZER_REFS if is_full_meta_repo() else {})

_attrs = ["cgo_enabled", "build_tags"]

_coverage_mode_refs = {
    "coverage_mode_atomic": "prelude//go/constraints:coverage_mode[atomic]",
    "coverage_mode_count": "prelude//go/constraints:coverage_mode[count]",
    "coverage_mode_set": "prelude//go/constraints:coverage_mode[set]",
}

go_binary_transition = transition(
    impl = _chain_transitions(_executable_transitions + [_coverage_mode_transition]),
    refs = _executable_refs | _coverage_mode_refs,
    attrs = _attrs + ["coverage_mode"],
)

go_test_transition = transition(
    impl = _chain_transitions(_executable_transitions + [_coverage_mode_transition]),
    refs = _executable_refs | _coverage_mode_refs,
    attrs = _attrs + ["coverage_mode"],
)

go_exported_library_transition = transition(
    impl = _chain_transitions(_top_level_transitions),
    refs = _top_level_refs,
    attrs = _attrs,
)

go_library_transition = transition(
    impl = _chain_transitions(_all_level_transitions),
    refs = _all_level_refs,
    attrs = [],
)

go_stdlib_transition = transition(
    impl = _chain_transitions(_all_level_transitions),
    refs = _all_level_refs,
    attrs = [],
)

cgo_enabled_attr = attrs.default_only(
    attrs.bool(
        default = select({
            "prelude//go/constraints:cgo_enabled[false]": False,
            "prelude//go/constraints:cgo_enabled[true]": True,
        })
    ),
)

coverage_mode_attr = attrs.default_only(
    attrs.option(
        attrs.enum(GoCoverageMode.values()),
        default = select({
            "prelude//go/constraints:coverage_mode[atomic]": "atomic",
            "prelude//go/constraints:coverage_mode[count]": "count",
            "prelude//go/constraints:coverage_mode[none]": None,
            "prelude//go/constraints:coverage_mode[set]": "set",
        }),
    )
)

build_tags_attr = attrs.default_only(attrs.list(attrs.string(), default = selects_for_tags()))
