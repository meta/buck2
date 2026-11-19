# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import os

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import (
    buck_test,
    BuckTestCase,
    get_mode_from_platform,
)


# This is just a template test case for `check_dependencies_test` to use buck2's e2e test framework.
# It does not need to be edited for new `check_dependencies_test`.

FLAVOR = os.environ["FLAVOR"]


def pass_list_arg(from_env_var: str, to_bxl_param: str) -> list[str]:
    list_env = os.environ[from_env_var]
    split_list = [] if list_env == "" else list_env.split(",")
    return [elem for item in split_list for elem in (to_bxl_param, item)]


class BxlCheckDependenciesTemplateTest(BuckTestCase):
    if FLAVOR == "check_dependencies_test":  # noqa: C901

        @buck_test(inplace=True)
        async def test_check_dependencies_bxl(self, buck: Buck) -> None:
            allowlist_args = pass_list_arg(
                from_env_var="ALLOWLIST", to_bxl_param="--allowlist_patterns"
            )
            blocklist_args = pass_list_arg(
                from_env_var="BLOCKLIST", to_bxl_param="--blocklist_patterns"
            )
            expect_failure_msg = os.environ["EXPECT_FAILURE_MSG"]

            mode_argfile = os.environ.get("CHECK_DEPENDENCIES_TEST_FBCODE_BUILD_MODE")
            if not mode_argfile:
                mode_argfile = get_mode_from_platform()

            additional_argfile = os.environ.get("EXTRA_BUCK_ARGS_FILE", None)
            additional_args = []
            if additional_argfile:
                additional_args.append(additional_argfile)

            bxl_call = buck.bxl(
                os.environ["BXL_MAIN"],
                mode_argfile,
                *additional_args,
                "--",
                "--target",
                os.environ["TARGET"],
                "--verification_mode",
                os.environ["VERIFICATION_MODE"],
                *allowlist_args,
                *blocklist_args,
                "--target_deps",
                os.environ["TARGET_DEPS"],
                *(
                    ["--deps_filter_pattern", os.environ["DEPS_FILTER_PATTERN"]]
                    if os.environ.get("DEPS_FILTER_PATTERN")
                    else []
                ),
                *(
                    ["--deps_exclude_pattern", os.environ["DEPS_EXCLUDE_PATTERN"]]
                    if os.environ.get("DEPS_EXCLUDE_PATTERN")
                    else []
                ),
                env={
                    "BUCK2_TEST_DISABLE_LOG_UPLOAD": "false",
                    "BUCK2_RUNTIME_THREADS": "8",
                    "BUCK2_MAX_BLOCKING_THREADS": "8",
                    # Cap jemalloc arenas: the default (4*ncpu) arenas each retain
                    # dirty pages rounded up to aarch64's 64KB page, inflating the
                    # buck2 daemon's DICE-graph RSS ~5x vs x86 (4KB pages). Cuts ARM
                    # peak ~68% to x86 parity, no x86 regression. See T286274456.
                    "MALLOC_CONF": "narenas:4,dirty_decay_ms:0,muzzy_decay_ms:0",
                    "_RJEM_MALLOC_CONF": "narenas:4,dirty_decay_ms:0,muzzy_decay_ms:0",
                },
            )
            if expect_failure_msg == "":
                await bxl_call
            else:
                await expect_failure(bxl_call, stderr_regex=expect_failure_msg)

    elif FLAVOR == "audit_dependents_test":

        @buck_test(inplace=True)
        async def test_audit_dependents_bxl(self, buck: Buck) -> None:
            allow_list = pass_list_arg(
                from_env_var="ALLOWLIST", to_bxl_param="--allowlist_patterns"
            )
            expect_failure_msg = os.environ["EXPECT_FAILURE_MSG"]

            bxl_call = buck.bxl(
                os.environ["BXL_MAIN"],
                "--",
                "--target",
                os.environ["TARGET"],
                "--source_target",
                os.environ["SOURCE_TARGET"],
                "--allowlist_patterns",
                *allow_list,
                env={
                    "BUCK2_TEST_DISABLE_LOG_UPLOAD": "false",
                },
            )
            if expect_failure_msg == "":
                await bxl_call
            else:
                await expect_failure(bxl_call, stderr_regex=expect_failure_msg)

    elif FLAVOR == "assert_dependencies_test":

        @buck_test(inplace=True)
        async def test_check_dependencies_bxl(self, buck: Buck) -> None:
            dep_list = pass_list_arg(from_env_var="DEPS", to_bxl_param="--deps")
            expect_failure_msg = os.environ["EXPECT_FAILURE_MSG"]
            additional_argfile = os.environ.get("EXTRA_BUCK_ARGS_FILE")
            additional_args = [additional_argfile] if additional_argfile else []
            bxl_call = buck.bxl(
                os.environ["BXL_MAIN"],
                *additional_args,
                "--",
                "--target",
                os.environ["TARGET"],
                *dep_list,
                env={
                    "BUCK2_TEST_DISABLE_LOG_UPLOAD": "false",
                },
            )
            if expect_failure_msg == "":
                await bxl_call
            else:
                await expect_failure(bxl_call, stderr_regex=expect_failure_msg)

    elif FLAVOR == "check_mutually_exclusive_dependencies_test":

        @buck_test(inplace=True)
        async def test_check_mutually_exclusive_dependencies_bxl(
            self, buck: Buck
        ) -> None:
            expect_failure_msg = os.environ["EXPECT_FAILURE_MSG"]

            # Build mode argfile is passed directly to buck2 as an argfile
            # e.g., "@fbsource//arvr/mode/android/linux/opt"
            build_mode_argfile = os.environ.get("BUILD_MODE_ARGFILE", "")
            additional_args = []
            if build_mode_argfile:
                additional_args.append(build_mode_argfile)

            bxl_call = buck.bxl(
                os.environ["BXL_MAIN"],
                *additional_args,
                "--",
                "--target",
                os.environ["TARGET"],
                "--mutually_exclusive_group",
                os.environ["MUTUALLY_EXCLUSIVE_GROUP"],
                "--target_deps",
                os.environ["TARGET_DEPS"],
                env={
                    "BUCK2_TEST_DISABLE_LOG_UPLOAD": "false",
                    "BUCK2_RUNTIME_THREADS": "8",
                    "BUCK2_MAX_BLOCKING_THREADS": "8",
                    # Cap jemalloc arenas: the default (4*ncpu) arenas each retain
                    # dirty pages rounded up to aarch64's 64KB page, inflating the
                    # buck2 daemon's DICE-graph RSS ~5x vs x86 (4KB pages). Cuts ARM
                    # peak ~68% to x86 parity, no x86 regression. See T286274456.
                    "MALLOC_CONF": "narenas:4,dirty_decay_ms:0,muzzy_decay_ms:0",
                    "_RJEM_MALLOC_CONF": "narenas:4,dirty_decay_ms:0,muzzy_decay_ms:0",
                },
            )
            if expect_failure_msg == "":
                await bxl_call
            else:
                await expect_failure(bxl_call, stderr_regex=expect_failure_msg)

    else:
        raise Exception(f"Found unknown test flavor: {FLAVOR}")
