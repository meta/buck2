# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load(":erlang_otp_application.bzl", "normalize_application")
load(":erlang_tests.bzl", "erlang_tests_macro")

# Macros

# Wrapper to generate the erlang_app and erlang_app_include target from a single
# specification. It also redirects the target from the regular application target
# to the include-only target for extra_include deps
def erlang_application(
    erlang_app_rule,
    erlang_app_includes_rule,
    name,
    app_name = None,
    applications = [],
    included_applications = [],
    extra_includes = [],
    labels = [],
    includes = [],
    **kwargs,
):
    normalized_applications = select_map(applications, lambda apps: map(normalize_application, apps))
    normalized_included_applications = select_map(included_applications, lambda apps: map(normalize_application, apps))

    if not includes:
        return erlang_app_rule(
            name = name,
            app_name = app_name,
            applications = normalized_applications,
            included_applications = normalized_included_applications,
            extra_includes = select_map(extra_includes, lambda deps: map(_extra_include_name, deps)),
            labels = labels,
            **kwargs,
        )
    else:
        return [
            erlang_app_includes_rule(
                name = _extra_include_name(name),
                app_name = name if app_name == None else app_name,
                includes = includes,
                visibility = kwargs.get("visibility", None),
                labels = ["generated", "app_includes"],
            ),
            erlang_app_rule(
                name = name,
                app_name = app_name,
                applications = normalized_applications,
                included_applications = normalized_included_applications,
                extra_includes = [_extra_include_name(dep) for dep in extra_includes],
                includes = includes,
                _includes_target = ":" + _extra_include_name(name),
                labels = labels,
                **kwargs,
            ),
        ]

# convenience macro to specify the includes-only target based on the base-application
# target name
def _extra_include_name(name: str) -> str:
    return "{}_includes_only".format(name)

def erlang_tests(
    erlang_app_rule,
    erlang_test_rule,
    suites: list[str] = [],
    deps: list[str] = [],
    resources: list[str] = [],
    srcs: list[str] = [],
    config_files: list[str] = [],
    common_app_env: dict[str, str] = {},
    **common_attributes,
):
    """
    Generate multiple erlang_test targets based on the `suites` field.
    """
    erlang_tests_macro(
        erlang_app_rule = erlang_app_rule,
        erlang_test_rule = erlang_test_rule,
        suites = suites,
        deps = deps,
        resources = resources,
        srcs = srcs,
        config_files = config_files,
        common_app_env = common_app_env,
        **common_attributes,
    )
