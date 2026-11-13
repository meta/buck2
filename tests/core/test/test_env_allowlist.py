# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, env

# `@env` sets these on the buck2 client, so the daemon it spawns inherits them.
# The `//:expect_*` targets then assert on what actually reaches the test
# process, which is what `[test] env_allowlist` controls.
PROBE = "BUCK2_E2E_ENV_PROBE"
PROBE_2 = "BUCK2_E2E_ENV_PROBE_2"

# The allowlist only applies to local execution, so pin every test to it.
LOCAL_ONLY = [
    "-c",
    "test.local_enabled=true",
    "-c",
    "test.remote_enabled=false",
]


@buck_test()
@env(PROBE, "probe-value")
async def test_env_not_inherited_by_default(buck: Buck) -> None:
    # The var is in the daemon's environment but not in the allowlist, so the
    # test process must not see it.
    await buck.test(*LOCAL_ONLY, "//:expect_unset")


@buck_test()
@env(PROBE, "probe-value")
async def test_env_inherited_when_allowlisted(buck: Buck) -> None:
    await buck.test(
        *LOCAL_ONLY,
        "-c",
        f"test.env_allowlist={PROBE}",
        "//:expect_set",
    )


@buck_test()
@env(PROBE, "probe-value")
@env(PROBE_2, "probe-value-2")
async def test_env_allowlist_accepts_a_list(buck: Buck) -> None:
    await buck.test(
        *LOCAL_ONLY,
        "-c",
        f"test.env_allowlist={PROBE},{PROBE_2}",
        "//:expect_both_set",
    )


@buck_test()
@env(PROBE, "probe-value")
@env(PROBE_2, "probe-value-2")
async def test_env_allowlist_only_covers_what_it_names(buck: Buck) -> None:
    # Allowlisting one var must not drag its neighbour along.
    await expect_failure(
        buck.test(
            *LOCAL_ONLY,
            "-c",
            f"test.env_allowlist={PROBE}",
            "//:expect_both_set",
        ),
    )


@buck_test()
@env(PROBE, "probe-value")
async def test_env_allowlist_ignores_unset_vars(buck: Buck) -> None:
    # Naming a var that isn't in the daemon's environment is not an error, and
    # must not show up in the test process as an empty string.
    await buck.test(
        *LOCAL_ONLY,
        "-c",
        "test.env_allowlist=BUCK2_E2E_ENV_PROBE_NEVER_SET",
        "//:expect_unset",
    )


@buck_test()
@env(PROBE, "probe-value")
async def test_changing_env_allowlist_reruns_the_test(buck: Buck) -> None:
    # Reading the config records a dep edge on it, so dropping the var from the
    # allowlist has to invalidate the successful run above rather than let the
    # daemon reuse it.
    await buck.test(
        *LOCAL_ONLY,
        "-c",
        f"test.env_allowlist={PROBE}",
        "//:expect_set",
    )
    await expect_failure(buck.test(*LOCAL_ONLY, "//:expect_set"))
