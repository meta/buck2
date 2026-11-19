# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import json
import tempfile
import unittest
from pathlib import Path

from type_check_result_to_validation import convert_type_check_result


class ConvertTypeCheckResultTest(unittest.TestCase):
    def _convert(self, type_check_result: object) -> dict[str, object]:
        with tempfile.TemporaryDirectory() as tmpdir:
            input_path = Path(tmpdir) / "type_check_result.json"
            output_path = Path(tmpdir) / "validation.json"
            input_path.write_text(json.dumps(type_check_result))
            convert_type_check_result(input_path, output_path)
            return json.loads(output_path.read_text())

    def test_no_errors_produces_success(self) -> None:
        self.assertEqual(
            self._convert({"errors": []}),
            {"version": 1, "data": {"status": "success"}},
        )

    def test_only_blocking_diagnostics_appear_in_failure(self) -> None:
        result = self._convert(
            {
                "errors": [
                    {"code": 0, "name": "unused-ignore"},
                    {"code": 0, "name": "other-error"},
                    {"code": 6, "name": "reveal-type", "severity": "info"},
                    {"code": 6, "name": "warning-kind", "severity": "warn"},
                    {"code": 6, "name": "ignored-error", "severity": "ignore"},
                    {
                        "code": 6,
                        "name": "incompatible-variable-type",
                        "severity": "error",
                        "path": "foo.py",
                        "line": 10,
                        "column": 5,
                        "description": "Expected int, got str",
                    },
                    {
                        "code": 7,
                        "name": "legacy-error",
                        "path": "bar.py",
                        "line": 20,
                        "column": 3,
                        "description": "Bad type",
                    },
                ]
            }
        )
        self.assertEqual(
            result,
            {
                "version": 1,
                "data": {
                    "status": "failure",
                    "message": (
                        "foo.py:10:5 Expected int, got str\nbar.py:20:3 Bad type"
                    ),
                },
            },
        )

    def test_unused_ignores_never_block(self) -> None:
        for name in ("unused-ignore", "unused-type-ignore"):
            with self.subTest(name=name):
                self.assertEqual(
                    self._convert(
                        {"errors": [{"code": -2, "name": name, "severity": "error"}]}
                    ),
                    {"version": 1, "data": {"status": "success"}},
                )

    def test_malformed_error_preserves_blocking_diagnostics(self) -> None:
        result = self._convert(
            {
                "errors": [
                    {"code": 6},
                    {
                        "code": 7,
                        "path": "foo.py",
                        "line": 10,
                        "column": 5,
                        "description": "Expected int, got str",
                    },
                ]
            }
        )
        self.assertEqual(
            result,
            {
                "version": 1,
                "data": {
                    "status": "failure",
                    "message": (
                        "foo.py:10:5 Expected int, got str\n"
                        "Malformed type checker output"
                    ),
                },
            },
        )

    def test_non_object_result_produces_failure(self) -> None:
        self.assertEqual(
            self._convert([]),
            {
                "version": 1,
                "data": {
                    "status": "failure",
                    "message": "Malformed type checker output",
                },
            },
        )
