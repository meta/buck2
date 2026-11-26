# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

from pathlib import Path

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import assert_executable, assert_not_executable
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


def _platform(name: str) -> list[str]:
    return ["--target-platforms", f"root//:{name}"]


async def _output(buck: Buck, target: str, platform: str) -> Path:
    """The content path `target` built for `platform` resolves to."""
    result = await buck.build(target, *_platform(platform))
    return result.get_build_report().output_for_target(target).resolve()


class ContentBasedPathsAcrossConfigurationsTest(BuckTestCase):
    @buck_test(skip_for_os=["windows"])
    async def test_local_action_cache_keys_on_the_executor(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        # `t` has the same command line and inputs in every configuration; its x86 and arm
        # configurations run on executors that differ in what can change a result.
        runs = tmp_path / "runs.log"
        runs.write_text("")
        (buck.cwd / "runs.bzl").write_text(f'RUNS = "{runs}"\n')

        await buck.build("root//:t", *_platform("x86_a"))
        assert len(runs.read_text().splitlines()) == 1
        # Another configuration on the same executor reuses the result.
        await buck.build("root//:t", *_platform("x86_b"))
        assert len(runs.read_text().splitlines()) == 1
        # A configuration on another executor executes again.
        await buck.build("root//:t", *_platform("arm_a"))
        assert len(runs.read_text().splitlines()) == 2

    @buck_test(
        skip_for_os=["windows"],
        extra_buck_config={
            "buck2": {"sqlite_materializer_state": "true", "sqlite_dep_file_state": "true"}
        }
    )
    async def test_persisted_local_action_cache_keys_on_the_executor(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        # The same as above, with a daemon restart before each build, so only the local action
        # cache persisted on disk can serve a result.
        runs = tmp_path / "runs.log"
        runs.write_text("")
        (buck.cwd / "runs.bzl").write_text(f'RUNS = "{runs}"\n')

        await buck.build("root//:t", *_platform("x86_a"))
        await buck.kill()
        await buck.build("root//:t", *_platform("x86_b"))
        assert len(runs.read_text().splitlines()) == 1, "the persisted entry was not reused"
        await buck.kill()
        await buck.build("root//:t", *_platform("arm_a"))
        assert len(runs.read_text().splitlines()) == 2

    @buck_test(skip_for_os=["windows"])
    async def test_executable_bit_separates_content_paths(self, buck: Buck) -> None:
        # The same bytes, executable on arm only: produced by a command, and by a write.
        for target in ["root//:r", "root//:w"]:
            exe = await _output(buck, target, "arm_a")
            plain = await _output(buck, target, "x86_a")
            assert exe != plain, (target, exe)
            assert_executable(exe)
            assert_not_executable(plain)
            # Building x86 did not flip the mode of arm's output.
            assert_executable(await _output(buck, target, "arm_a"))
