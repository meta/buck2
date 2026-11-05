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

        for field in [
            "configured_graph_sketch",
            "retained_analysis_memory_sketch",
            "action_graph_sketch",
            "artifact_count_sketch",
            "artifact_size_sketch",
            "artifact_count_sketch_cardinality",
            "artifact_size_sketch_cardinality",
        ]:
            assert (
                field
                not in report["results"]["root//:ok"]["configured"]["<unspecified>"]
            )

    @buck_test()
    async def test_build_and_test_reports_have_matching_sketches(
        self, buck: Buck, tmp_path: Path
    ) -> None:
        build_report = tmp_path / "build-report.json"
        test_report = tmp_path / "test-report.json"
        configs = [
            "-c",
            "buck2.log_configured_graph_size=true",
            "-c",
            "buck2.log_configured_graph_sketch=true",
            "-c",
            "buck2.log_retained_analysis_memory_sketch=true",
            "-c",
            "buck2.log_action_graph_sketch=true",
            "-c",
            "buck2.log_artifact_count_sketch=true",
            "-c",
            "buck2.log_artifact_size_sketch=true",
            "-c",
            "buck2.log_sketch_cardinalities=true",
            "-c",
            "buck2.providers_to_skip_in_artifact_path_sketch=build,run",
        ]

        await buck.build(
            "//:ok",
            "--build-default-info",
            "--skip-run-info",
            "--build-test-info",
            *configs,
            "--build-report",
            str(build_report),
        )
        await buck.test(
            "//:ok",
            "--build-default-info",
            *configs,
            "--build-report",
            str(test_report),
        )

        with open(build_report) as file:
            build_entry = json.load(file)["results"]["root//:ok"]["configured"][
                "<unspecified>"
            ]
        with open(test_report) as file:
            test_entry = json.load(file)["results"]["root//:ok"]["configured"][
                "<unspecified>"
            ]

        assert build_entry["configured_graph_size"] > 0
        assert (
            test_entry["configured_graph_size"] == build_entry["configured_graph_size"]
        )

        for field in [
            "configured_graph_sketch",
            "retained_analysis_memory_sketch",
            "action_graph_sketch",
            "artifact_count_sketch",
            "artifact_size_sketch",
        ]:
            assert build_entry[field].startswith("V1:")
            assert test_entry[field] == build_entry[field]

        for field in [
            "artifact_count_sketch_cardinality",
            "artifact_size_sketch_cardinality",
        ]:
            assert build_entry[field] > 0
            assert test_entry[field] == build_entry[field]
