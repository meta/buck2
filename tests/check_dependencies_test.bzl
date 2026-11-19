# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

load("@fbcode//buck2/tests:buck_e2e.bzl", "buck2_e2e_test")
load("@fbcode_macros//build_defs:native_rules.bzl", "buck_genrule")
load("@fbsource//tools/build_defs/testinfra:network_access_utils.bzl", "network_access_utils")
load("@fbsource//tools/build_defs/windows:powershell.bzl", "powershell_cmd_exe")
load("@fbsource//tools/target_determinator/macros:ci.bzl", "ci")

# This is meant to be Open-source friendly. In our e2e tests, we invoke a variant from
# tools/build_defs/check_dependencies_test.bzl that passes additional arguments for meta specific allowlist.

def _check_dependencies_test(name, target, contacts, env, labels: list[str], deps, compatible_with = None, **kwargs):
    # The test module's name comes from `base_module`; leaving it to the calling
    # package lets it collide with a stdlib module (e.g. a package named `cmd`).
    kwargs.setdefault("base_module", "buck2.tests.e2e_util")
    buck2_e2e_test(
        contacts = contacts,
        name = name,
        srcs = {"fbcode//buck2/tests/e2e_util:test_bxl_check_dependencies_template.py": "test_bxl_check_dependencies_template.py"},
        env = env,
        heavyweight_threads = "8",
        labels = labels,
        test_with_compiled_buck2 = False,
        test_with_deployed_buck2 = True,
        skip_deployed_buck2_version_dep = True,
        use_buck_api = False,
        # In order for target determinator to trigger this test when the `target` specified has changed, we need to introduce a dep on `target`.
        # However, we cannot introduce a configured dep, because the `target` may not be compatible with platform of dependencies test.
        # This adds a dep on `target` in a select arm that is never satisfied. This will work for TD because TD only looks at deps on unconfigured
        # target graph.
        deps = (deps or [])
        + select({
            "DEFAULT": [],
            "ovr_config//:none": [target],
        }),
        compatible_with = compatible_with,
        **kwargs,
    )

def _extra_buck_args_file(name, extra_buck_args):
    extra_buck_args_target = "%s_extra_buck_args" % (name)
    buck_args_str = "\n".join(extra_buck_args)
    buck_genrule(
        name = extra_buck_args_target,
        out = "extra_buck_args",
        bash = "echo '%s' > $OUT" % (buck_args_str),
        cmd_exe = powershell_cmd_exe([
            "Set-Content $OUT @(%s)" % (", ".join(["'%s'" % arg.replace("'", "''") for arg in extra_buck_args])),
        ]),
    )
    return "@$(location :%s)" % (extra_buck_args_target)

def check_dependencies_test(
    name,
    target,
    contacts,
    mode,
    allowlist_patterns = None,
    blocklist_patterns = None,
    expect_failure_msg = None,
    env = None,
    deps = None,
    extra_buck_args = [],
    labels = [],
    target_deps = True,
    deps_filter_pattern = None,
    deps_exclude_pattern = None,
    **kwargs,
):
    """
    Creates a test target from a buck2 bxl script. BXL script must use "test" as entry
    point.

    There are two modes: "allowlist" mode, "blocklist" mode

    "allowlist" mode:

        Only deps matching patterns in "allowlist_patterns' will pass the check.
        If at least one target in a transitive closure of all dependencies doesn't
        match anything in "allowlist_patterns" the test will fail.

        In this mode "blocklist_patterns" is used to allow for additional restriction
        to be applied. If a specific target from transitive closure of all dependencies
        matches at least one pattern from "blocklist_patterns", the test will fail
        regardless if that target matches anything in "allowlist_patterns".

        In other words, for test to pass each target must match at least one
        pattern from "allowlist_patterns", and must not match anything from the
        "blocklist_patterns".

    "blocklist" mode"

        If at least one target from a transitive closure of dependnecies matches
        at least one pattern from "blocklist_patterns" the test will fail.

        In this mode "allowlist_patterns" modifies how "blocklist_patterns" are
        applied. Specifically if a specific target matches "allowlist_patterns"
        then it will always pass, and will not be checked against "blocklist_patterns".

        The use case is to allow blocking more generic location (for example,
        blocklist; //testing/.*), while still allowing specific exceptions
        (for example, allowlist: //testing/jest/.*).
    """

    kwargs.setdefault("network_access", network_access_utils.all())
    bxl_main = "fbcode//buck2/tests/check_dependencies_test.bxl:test"
    allowlist_patterns = ",".join(allowlist_patterns) if allowlist_patterns else ""
    blocklist_patterns = ",".join(blocklist_patterns) if blocklist_patterns else ""
    if not (expect_failure_msg == None or len(expect_failure_msg) > 0):
        fail("Expected failure message can only be None or non-empty string")

    if mode not in ("allowlist", "blocklist"):
        fail("mode must be one of: allowlist, blocklist")

    modifiers = kwargs.pop("modifiers", None)
    if modifiers:
        for m in modifiers:
            extra_buck_args = extra_buck_args + ["--modifier", m]

    extra_buck_args_file = _extra_buck_args_file(name, extra_buck_args)

    _check_dependencies_test(
        contacts = contacts,
        name = name,
        target = target,
        env = {
            "ALLOWLIST": allowlist_patterns,
            "BLOCKLIST": blocklist_patterns,
            "BXL_MAIN": bxl_main,
            "DEPS_EXCLUDE_PATTERN": deps_exclude_pattern or "",
            "DEPS_FILTER_PATTERN": deps_filter_pattern or "",
            "EXPECT_FAILURE_MSG": expect_failure_msg or "",
            "EXTRA_BUCK_ARGS_FILE": extra_buck_args_file,
            "FLAVOR": "check_dependencies_test",
            "TARGET": target,
            "TARGET_DEPS": str(target_deps).lower(),
            "VERIFICATION_MODE": mode,
        }
        | (env or {}),
        labels = ["check_dependencies_test"] + labels,
        deps = deps,
        **kwargs,
    )

def assert_dependencies_test(name, target, contacts, expected_deps, expect_failure_msg = None, deps = None, labels = [], extra_buck_args = [], **kwargs):
    """
    Creates a test target fromfbcode//buck2/tests/assert_dependencies_test.bxl:test bxl script.

    Parameters:
        name: Name of the test target.
        contacts: List of oncalls for the test.
        target: The target to check dependencies for
        expected_deps: list of expected deps
        extra_buck_args: Additional Buck arguments used for the BXL invocation.
    """
    extra_env = {"EXTRA_BUCK_ARGS_FILE": _extra_buck_args_file(name, extra_buck_args)} if extra_buck_args else {}
    _check_dependencies_test(
        name = name,
        target = target,
        contacts = contacts,
        env = {
            "BXL_MAIN": "fbcode//buck2/tests/assert_dependencies_test.bxl:test",
            "DEPS": ",".join(expected_deps),
            "EXPECT_FAILURE_MSG": expect_failure_msg or "",
            "FLAVOR": "assert_dependencies_test",
            "TARGET": target,
        }
        | extra_env,
        labels = labels + ["assert_dependencies_test"],
        deps = deps,
        **kwargs,
    )

def audit_dependents_test(name, target, contacts, source_target, allowlist_patterns, expect_failure_msg = None, deps = None, **kwargs):
    """
    Creates a test target from a buck2 bxl script. BXL script must use "test" as entry
    point.

    Parameters:
        name: Name of the test target.
        contacts: List of oncalls for the test.
        target: The target to check direct dependents for
        source_target: The target universe
        allowlist_patter: a regex of patterns that should be allowed for direct dependents of target
        expect_failure_msg: the test is expected to fail with this message regex
    """
    _check_dependencies_test(
        name = name,
        target = target,
        contacts = contacts,
        env = {
            "ALLOWLIST": ",".join(allowlist_patterns) if allowlist_patterns else "",
            "BXL_MAIN": "fbcode//buck2/tests/audit_dependents_test.bxl:test",
            "EXPECT_FAILURE_MSG": expect_failure_msg or "",
            "FLAVOR": "audit_dependents_test",
            "SOURCE_TARGET": source_target,
            "TARGET": target,
        },
        labels = ["audit_dependents_test"],
        deps = deps,
        **kwargs,
    )

def check_mutually_exclusive_dependencies_test(
    name, target, contacts, mutually_exclusive_group, expect_failure_msg = None, deps = None, labels = [], target_deps = True, build_mode = None, **kwargs
):
    """
    Creates a test target from a buck2 bxl script that checks for mutually exclusive dependencies.

    This test verifies that the target does not depend on more than one dependency from the
    mutually exclusive group. For example, if your group is:
    [//third-party/volk:volk, //third-party/volk:volk-header, //third-party/toolchains:vulkan]

    The test ensures at most one of these patterns has matches in the dependency tree.

    Parameters:
        name: Name of the test target.
        contacts: List of oncalls for the test.
        target: The target to check dependencies for
        mutually_exclusive_group: List of dependency patterns where only one should be present.
            Each pattern can be a specific target (e.g., "//foo/bar:baz") or a regex pattern
            (e.g., "//foo/.*").
            Example: ["//third-party/volk:volk", "//third-party/volk:volk-header"]
        expect_failure_msg: Optional regex pattern for expected failure message
        deps: Optional list of additional dependencies
        labels: Optional list of labels for the test
        target_deps: If True, only check target_deps() (default: True)
        build_mode: Optional build mode flagfile for the BXL cquery. Use this to analyze
            dependencies for a specific platform (e.g., Android) while running the test on Linux.
            When specified, CI labels are automatically set to run the test only once on Linux.
            Example: "fbsource//arvr/mode/android/linux/opt"
    """

    # Convert list to comma-separated string for BXL
    group_str = ",".join(mutually_exclusive_group)

    # Build mode flagfile is passed directly to buck2 as an argfile
    # The flagfile contains --target-platforms and other config flags
    build_mode_argfile = ""
    ci_labels = []
    if build_mode:
        # Ensure the build mode has the @ prefix for argfile syntax
        if build_mode.startswith("@"):
            build_mode_argfile = build_mode
        else:
            build_mode_argfile = "@" + build_mode

        # When build_mode is specified, the test will produce the same result
        # regardless of which Linux CI mode runs it. Add CI labels to run only once.
        ci_labels = [ci.overwrite(), ci.linux()]

    _check_dependencies_test(
        name = name,
        target = target,
        contacts = contacts,
        env = {
            "BUILD_MODE_ARGFILE": build_mode_argfile,
            "BXL_MAIN": "fbcode//buck2/tests/check_mutually_exclusive_dependencies_test.bxl:test",
            "EXPECT_FAILURE_MSG": expect_failure_msg or "",
            "FLAVOR": "check_mutually_exclusive_dependencies_test",
            "MUTUALLY_EXCLUSIVE_GROUP": group_str,
            "TARGET": target,
            "TARGET_DEPS": str(target_deps).lower(),
        },
        labels = ci_labels + labels + ["check_mutually_exclusive_dependencies_test"],
        deps = deps,
        # The test binary uses Python/pytest which doesn't work on platforms like android
        compatible_with = ["ovr_config//os:linux", "ovr_config//os:macos", "ovr_config//os:windows"],
        **kwargs,
    )
