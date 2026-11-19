#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import os
import tempfile
import unittest
from unittest.mock import patch

from buck2.tests.e2e_util.helper import golden


def _check(expected: str, actual: str) -> None:
    """Writes `expected` as the golden file, then checks `actual` against it."""
    with tempfile.TemporaryDirectory() as src:
        env = {"TEST_REPO_DATA_SRC": src}
        with patch.dict(os.environ, {**env, "BUCK2_UPDATE_GOLDEN": "1"}):
            golden.golden(output=expected, rel_path="golden/out.golden.json")
        with patch.dict(os.environ, env):
            golden.golden(output=actual, rel_path="golden/out.golden.json")


class GoldenCiLabelTest(unittest.TestCase):
    def test_a_plain_change_fails(self) -> None:
        with self.assertRaises(AssertionError):
            _check('[\n  "root//tools:lint"\n]', '[\n  "root//tools:fmt"\n]')

    def test_ci_labels_are_ignored(self) -> None:
        _check('[\n  "ci:overwrite",\n  "ci:diff:linux",\n  "foo"\n]', '[\n  "foo"\n]')

    def test_a_changed_target_in_a_ci_package_fails(self) -> None:
        with self.assertRaises(AssertionError):
            _check('[\n  "root//tools/ci:lint"\n]', '[\n  "root//tools/ci:fmt"\n]')

    def test_a_lost_dependency_in_a_ci_package_fails(self) -> None:
        with self.assertRaises(AssertionError):
            _check("deps:\n  root//a:a\n  root//ci:lint\n", "deps:\n  root//a:a\n")


class SanitizeRemoteActionOriginTest(unittest.TestCase):
    def test_re_served_and_deduplicated_read_as_executed(self) -> None:
        executed = (
            "Remote action, reproduce with: `frecli cas download-action <DIGEST>`"
        )
        for origin in [
            "served from the remote execution cache",
            "deduplicated by remote execution",
        ]:
            self.assertEqual(
                golden.sanitize_remote_action_origin(
                    f"Remote action {origin}, reproduce with: `frecli cas download-action <DIGEST>`"
                ),
                executed,
            )
        self.assertEqual(golden.sanitize_remote_action_origin(executed), executed)

    def test_buck2_cache_hit_is_kept(self) -> None:
        s = "Remote action cache hit, reproduce with: `frecli cas download-action <DIGEST>`"
        self.assertEqual(golden.sanitize_remote_action_origin(s), s)

    def test_build_report_strings_are_sanitized(self) -> None:
        report = {
            "trace_id": "",
            "project_root": "",
            "failures": {
                "root//:a": "Remote action deduplicated by remote execution, reproduce with: x"
            },
        }
        golden.sanitize_build_report(report)
        self.assertEqual(
            report["failures"], {"root//:a": "Remote action, reproduce with: x"}
        )
