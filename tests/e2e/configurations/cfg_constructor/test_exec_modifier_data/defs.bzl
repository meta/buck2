# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Custom rules for testing exec platform modifiers."""

load("@prelude//cfg/exec_platform:marker.bzl", "get_exec_platform_marker")
load("@prelude//cfg/modifier:cfg_constructor.bzl", "cfg_constructor_post_constraint_analysis", "cfg_constructor_pre_constraint_analysis")

_FALSE_EXEC_MARKER = "prelude//cfg/exec_platform/marker:is_exec_platform[false]"

def cfg_constructor_with_exec_marker_refs(**kwargs):
    refs, params = cfg_constructor_pre_constraint_analysis(**kwargs)
    if params.extra_data.replace_exec_marker:
        refs.append(_FALSE_EXEC_MARKER)
    return refs, params

def cfg_constructor_changing_exec_marker(*, refs, params):
    platform = cfg_constructor_post_constraint_analysis(refs = refs, params = params)
    if not params.configuring_exec_dep:
        return platform

    marker = refs[get_exec_platform_marker()][ConstraintValueInfo]
    constraints = {key: value for key, value in platform.configuration.constraints.items() if key != marker.setting.label}
    if params.extra_data.replace_exec_marker:
        constraints[marker.setting.label] = refs[_FALSE_EXEC_MARKER][ConstraintValueInfo]
    return PlatformInfo(
        label = platform.label,
        configuration = ConfigurationInfo(
            constraints = constraints,
            values = platform.configuration.values,
        ),
    )

def get_labels():
    """Returns labels based on build_mode and compiler constraints."""
    return (
        select({
            "//cfg:build_mode[debug]": ["build_mode:debug"],
            "//cfg:build_mode[none]": ["build_mode:none"],
            "//cfg:build_mode[release]": ["build_mode:release"],
        })
        + select({
            "//cfg:compiler[clang]": ["compiler:clang"],
            "//cfg:compiler[gcc]": ["compiler:gcc"],
            "//cfg:compiler[none]": ["compiler:none"],
        })
        + select({
            "//cfg:os[linux]": ["os:linux"],
            "//cfg:os[macos]": ["os:macos"],
            "//cfg:os[none]": ["os:none"],
        })
        + select({
            "//cfg:cpu[arm64]": ["cpu:arm64"],
            "//cfg:cpu[none]": ["cpu:none"],
            "//cfg:cpu[x86_64]": ["cpu:x86_64"],
        })
    )

def get_buckconfig_backed_label():
    return select({
        "//cfg:buckconfig_backed[enabled]": "buckconfig_backed:enabled",
        "//cfg:buckconfig_backed[none]": "buckconfig_backed:none",
    })

def _dummy(ctx):
    _ignore = ctx
    return [
        DefaultInfo(),
    ]

# Rule that has dependencies and does nothing. Useful for query-like tests.
dummy = rule(
    impl = _dummy,
    attrs = {
        "buckconfig_backed_label": attrs.string(default = ""),
        "configured_deps": attrs.list(attrs.configured_dep(), default = []),
        "deps": attrs.list(attrs.dep(), default = []),
        "exec_deps": attrs.list(attrs.exec_dep(), default = []),
        "is_exec": attrs.string(default = ""),
        "labels": attrs.list(attrs.string(), default = []),
        "srcs": attrs.list(attrs.source(), default = []),
        "toolchain_deps": attrs.list(attrs.toolchain_dep(), default = []),
    },
)

def labeled_dummy(name, **kwargs):
    """
    A wrapper around dummy that automatically sets labels based on constraints.

    Args:
        name: The name of the target
        **kwargs: Additional arguments passed to dummy()
    """

    # Set labels if not already provided
    if "labels" not in kwargs:
        kwargs["labels"] = get_labels()

    if "buckconfig_backed_label" not in kwargs:
        kwargs["buckconfig_backed_label"] = get_buckconfig_backed_label()

    # Set default_target_platform if not provided
    if "default_target_platform" not in kwargs:
        kwargs["default_target_platform"] = "//cfg:debug_platform"

    dummy(name = name, **kwargs)

def labeled_tool(name, **kwargs):
    """
    A wrapper around dummy specifically for build tools used as exec_deps.
    Automatically sets labels based on constraints.
    """

    # Set labels if not already provided
    if "labels" not in kwargs:
        kwargs["labels"] = get_labels()

    dummy(name = name, **kwargs)

def _toolchain_impl(ctx):
    _ignore = ctx
    return [
        DefaultInfo(),
    ]

# Toolchain rule - marked with is_toolchain_rule = True
toolchain = rule(
    impl = _toolchain_impl,
    attrs = {
        "additional_tools": attrs.list(attrs.exec_dep(), default = []),  # Additional tools for testing multiple exec deps
        "labels": attrs.list(attrs.string(), default = []),
        "tool": attrs.option(attrs.exec_dep(), default = None),  # The actual tool that runs
    },
    is_toolchain_rule = True,
)

def labeled_toolchain(name, **kwargs):
    """
    A wrapper around toolchain that automatically sets labels.
    """

    # Set labels if not already provided
    if "labels" not in kwargs:
        kwargs["labels"] = get_labels()

    toolchain(name = name, **kwargs)

# ============================================================================
# Transitions for testing exec marker preservation across transitions
# ============================================================================

def _single_transition_impl(platform, refs, attrs):
    """Choose a compiler using the input marker, optionally returning a conflicting marker."""
    marker = refs.exec_marker[ConstraintValueInfo]
    input_marker = platform.configuration.constraints.get(marker.setting.label)
    is_exec = input_marker != None and input_marker.label == marker.label
    c = (refs.clang if is_exec else refs.gcc)[ConstraintValueInfo]
    constraints = {c.setting.label: c}
    if attrs.set_false_marker:
        constraints[marker.setting.label] = refs.false_marker[ConstraintValueInfo]
    return PlatformInfo(
        label = "single_transitioned",
        configuration = ConfigurationInfo(
            constraints = constraints,
            values = {},
        ),
    )

_single_transition = transition(
    impl = _single_transition_impl,
    attrs = ["set_false_marker"],
    refs = {
        "clang": "//cfg:compiler[clang]",
        "exec_marker": "prelude//cfg/exec_platform/marker:is_exec_platform[true]",
        "false_marker": _FALSE_EXEC_MARKER,
        "gcc": "//cfg:compiler[gcc]",
    },
)

def _split_transition_impl(platform, refs, attrs):
    """Return fresh split configurations, optionally with a conflicting marker in b."""
    _ = platform  # buildifier: disable=unused-variable
    c_a = refs.constraint_a[ConstraintValueInfo]
    c_b = refs.constraint_b[ConstraintValueInfo]
    constraints_b = {c_b.setting.label: c_b}
    if attrs.set_false_marker:
        marker = refs.false_marker[ConstraintValueInfo]
        constraints_b[marker.setting.label] = marker

    return {
        "a": PlatformInfo(
            label = "split_transitioned_a",
            configuration = ConfigurationInfo(
                constraints = {c_a.setting.label: c_a},
                values = {},
            ),
        ),
        "b": PlatformInfo(
            label = "split_transitioned_b",
            configuration = ConfigurationInfo(
                constraints = constraints_b,
                values = {},
            ),
        ),
    }

_split_transition = transition(
    impl = _split_transition_impl,
    attrs = ["set_false_marker"],
    refs = {
        "constraint_a": "//cfg:compiler[gcc]",
        "constraint_b": "//cfg:compiler[clang]",
        "false_marker": _FALSE_EXEC_MARKER,
    },
    split = True,
)

# Select expression that resolves to "true" or "false" based on the exec platform marker.
# Used to directly verify the marker is present in the configuration.
def get_exec_marker_label():
    return select({
        "DEFAULT": "is_exec:false",
        "prelude//cfg/exec_platform/marker:is_exec_platform[true]": "is_exec:true",
    })

# Rule that wraps a transition dep - used as an exec dep to test marker preservation.
# When this rule is used as an exec_dep, it's configured with the exec platform config.
# Its transition_dep then receives the exec platform config as input.
def _tool_with_transition_dep_impl(_ctx):
    return [DefaultInfo()]

tool_with_single_transition_dep = rule(
    impl = _tool_with_transition_dep_impl,
    attrs = {
        "is_exec": attrs.string(default = ""),
        "labels": attrs.list(attrs.string(), default = []),
        "set_false_marker": attrs.bool(default = False),
        "transitioned_dep": attrs.transition_dep(cfg = _single_transition),
    },
)

tool_with_split_transition_dep = rule(
    impl = _tool_with_transition_dep_impl,
    attrs = {
        "is_exec": attrs.string(default = ""),
        "labels": attrs.list(attrs.string(), default = []),
        "set_false_marker": attrs.bool(default = False),
        "transitioned_dep": attrs.split_transition_dep(cfg = _split_transition),
    },
)
