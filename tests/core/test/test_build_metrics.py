# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import json
from pathlib import Path

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


class BuildMetricsTest(BuckTestCase):
    @buck_test()
    async def test_build_report_contains_per_target_build_metrics(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        report = tmp_path / "build-report.json"

        # `--build-default-info` is what a combined build-and-test invocation passes,
        # and it is what gives the target a build output to time.
        await buck.test(
            "//:ok",
            "--build-default-info",
            "-c",
            "buck2.detailed_aggregated_metrics=true",
            "--build-report",
            str(report),
        )

        with open(report) as file:
            report = json.load(file)

        # Values change across runs, so assert the fields are populated rather than
        # goldening them.
        assert report["build_metrics"]

        metrics = report["results"]["root//:ok"]["configured"]["<unspecified>"][
            "build_metrics"
        ]
        assert metrics["metrics"]["declared_actions"] > 0
        # Measured from the start of the command, as `buck build` reports it, rather
        # than from the start of this target's own build.
        assert metrics["wall_clock_completion_ms"] > 0
