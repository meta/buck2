#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import unittest

from buck2.tests.e2e_util.api.buck_result import parse_trace_id

A = "d69746cb-6d31-4c16-b546-7ca24c4ab759"
B = "5717fe72-ed95-4542-b038-6574236410c8"


class ParseTraceIdTest(unittest.TestCase):
    def test_buck_ui_line(self) -> None:
        stderr = f"Buck UI: https://www.internalfb.com/buck2/{A}\nBUILD SUCCEEDED\n"
        self.assertEqual(parse_trace_id(stderr), A)

    def test_build_id_line(self) -> None:
        self.assertEqual(parse_trace_id(f"Build ID: {A}\n"), A)

    def test_timestamp_prefix(self) -> None:
        stderr = f"[2026-10-27T09:35:12.103-07:00] Buck UI: https://www.internalfb.com/buck2/{A}\n"
        self.assertEqual(parse_trace_id(stderr), A)

    def test_restart_takes_last_attempt(self) -> None:
        stderr = (
            f"Buck UI: https://www.internalfb.com/buck2/{A}\n"
            "Your command will now restart.\n"
            f"Buck UI: https://www.internalfb.com/buck2/{B}\n"
        )
        self.assertEqual(parse_trace_id(stderr), B)

    def test_ignores_indented_and_embedded_mentions(self) -> None:
        stderr = (
            f"Buck UI: https://www.internalfb.com/buck2/{A}\n"
            f"    Buck UI: https://www.internalfb.com/buck2/{B}\n"
            f"see https://www.internalfb.com/buck2/{B}\n"
        )
        self.assertEqual(parse_trace_id(stderr), A)

    def test_no_line(self) -> None:
        self.assertIsNone(parse_trace_id("Error: Could not read dotslash file\n"))
        self.assertIsNone(parse_trace_id(""))
