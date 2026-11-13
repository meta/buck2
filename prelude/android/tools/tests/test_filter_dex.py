# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Tests for filter_dex.py.

Covers primary-dex membership for the synthetic classes D8 creates. The synthetic names used
here deliberately do not resemble D8's actual mangling -- membership must come from the
synthetic-to-context map D8 reports, never from the shape of the name.
"""

from __future__ import annotations

import json
import os
import pathlib
import sys
import tempfile
import unittest
from unittest.mock import patch

from android.tools.filter_dex import (
    _belongs_in_primary_dex,
    _parse_dex_units,
    _parse_ref_counts,
    _parse_synthetic_contexts,
    _resolve_synthesizing_context,
    ClassNameFilter,
    main,
)


class ParseRefCountsTest(unittest.TestCase):
    def _parse(self, contents: str):
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "ref_count.txt")
            with open(path, "w") as f:
                f.write(contents)
            return _parse_ref_counts(path)

    def test_lib_totals_alone(self) -> None:
        self.assertEqual(self._parse("19 10 17\n"), ("19", "10", "17", {}))

    def test_counts_of_each_class(self) -> None:
        self.assertEqual(
            self._parse("124 72367 53\n2 16405 4 a/P3\n13 4 12 a/B$C\n"),
            (
                "124",
                "72367",
                "53",
                {"a/P3.class": [2, 16405, 4], "a/B$C.class": [13, 4, 12]},
            ),
        )

    def test_malformed_class_line_discards_all_class_counts(self) -> None:
        for line in ("2 3 4", "2 3 4 a/With Space", "x 3 4 a/B", "", "   "):
            with self.subTest(line=line):
                self.assertEqual(
                    self._parse(f"124 72367 53\n2 16405 4 a/P3\n{line}\n13 4 12 a/B\n"),
                    ("124", "72367", "53", {}),
                )

    def test_malformed_lib_totals_are_rejected(self) -> None:
        with self.assertRaises(ValueError):
            self._parse("19 10\n2 3 4 a/B\n")


class ParseSyntheticContextsTest(unittest.TestCase):
    def _parse(self, contents: str) -> dict[str, str]:
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "synthetic_contexts.txt")
            with open(path, "w") as f:
                f.write(contents)
            return _parse_synthetic_contexts(path)

    def test_parses_pairs(self) -> None:
        self.assertEqual(
            self._parse("a/zzz0 a/Foo\nb/zzz1 b/Bar\n"),
            {"a/zzz0": "a/Foo", "b/zzz1": "b/Bar"},
        )

    def test_empty_file_yields_empty_map(self) -> None:
        self.assertEqual(self._parse(""), {})

    def test_blank_lines_are_skipped(self) -> None:
        self.assertEqual(self._parse("\na/zzz0 a/Foo\n\n"), {"a/zzz0": "a/Foo"})

    def test_line_without_context_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            self._parse("a/zzz0\n")


class ResolveSynthesizingContextTest(unittest.TestCase):
    def test_non_synthetic_resolves_to_itself(self) -> None:
        self.assertEqual(_resolve_synthesizing_context("a/Foo", {}), "a/Foo")

    def test_single_hop(self) -> None:
        self.assertEqual(
            _resolve_synthesizing_context("a/zzz0", {"a/zzz0": "a/Foo"}), "a/Foo"
        )

    def test_chained_synthetics_resolve_to_the_root_context(self) -> None:
        # D8 can synthesize from a synthetic, so the chain has to be walked to its end.
        chain = {"a/zzz1": "a/zzz0", "a/zzz0": "a/Foo"}
        self.assertEqual(_resolve_synthesizing_context("a/zzz1", chain), "a/Foo")

    def test_cycle_terminates(self) -> None:
        cycle = {"a/zzz0": "a/zzz1", "a/zzz1": "a/zzz0"}
        self.assertIn(_resolve_synthesizing_context("a/zzz0", cycle), cycle)


class BelongsInPrimaryDexTest(unittest.TestCase):
    def _belongs(self, java_class, patterns, synthetic_to_context=None) -> bool:
        return _belongs_in_primary_dex(
            java_class, ClassNameFilter(patterns), synthetic_to_context or {}
        )

    def test_class_matching_a_prefix_pattern(self) -> None:
        self.assertTrue(self._belongs("a/Foo", ["^a/"]))

    def test_class_matching_a_suffix_pattern(self) -> None:
        self.assertTrue(self._belongs("a/Foo", ["Foo^"]))

    def test_class_matching_a_substring_pattern(self) -> None:
        self.assertTrue(self._belongs("a/Foo", ["/Fo"]))

    def test_class_matching_a_regex_pattern(self) -> None:
        self.assertTrue(self._belongs("a/Foo", ["^-a/F.*"]))

    def test_unmatched_class_stays_out(self) -> None:
        self.assertFalse(self._belongs("b/Bar", ["^a/"]))

    def test_synthetic_follows_its_context_into_the_primary_dex(self) -> None:
        self.assertTrue(self._belongs("b/zzz0", ["^a/"], {"b/zzz0": "a/Foo"}))

    def test_synthetic_of_a_secondary_class_stays_out(self) -> None:
        self.assertFalse(self._belongs("b/zzz0", ["^a/"], {"b/zzz0": "b/Bar"}))

    def test_chained_synthetic_follows_its_root_context(self) -> None:
        chain = {"b/zzz1": "b/zzz0", "b/zzz0": "a/Foo"}
        self.assertTrue(self._belongs("b/zzz1", ["^a/"], chain))

    def test_pattern_naming_a_synthetic_directly_still_matches(self) -> None:
        # Some primary_dex_patterns name a D8 synthetic outright. Those must keep working even
        # though the synthetic's context is not itself in the primary dex.
        self.assertTrue(
            self._belongs(
                "androidx/Foo$ExternalSyntheticLambda0",
                ["^androidx/Foo$ExternalSyntheticLambda0^"],
                {"androidx/Foo$ExternalSyntheticLambda0": "androidx/Foo"},
            )
        )

    def test_regex_pattern_matches_at_the_start_of_the_name(self) -> None:
        # Same answer as the Java `ClassNameFilter` that non-pre-dexed builds use.
        self.assertFalse(self._belongs("com/app/MainActivity", ["^-Activity"]))
        self.assertTrue(self._belongs("com/app/MainActivity", ["^-com/app/.*Activity"]))


class ParseDexUnitsTest(unittest.TestCase):
    def _parse(self, contents: str, class_names: list[str] | None = None):
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / "units.tsv"
            path.write_text(contents)
            return _parse_dex_units(
                path,
                class_names if class_names is not None else ["a/Foo"],
                ClassNameFilter([]),
                {},
            )

    def test_parses_counts_and_all_members_in_producer_order(self) -> None:
        units = self._parse(
            "a/Foo\t123\t11\t12\t13\tz/one\ta/Foo\tz/two\n"
            "a/Other\t0\t0\t0\t0\ta/Other\n",
            ["a/Foo", "z/one", "z/two", "a/Other"],
        )
        self.assertEqual(
            units,
            [
                {
                    "class_names": ["z/one.class", "a/Foo.class", "z/two.class"],
                    "primary": False,
                    "weight_estimate": 123,
                    "method_ref_count": 11,
                    "field_ref_count": 12,
                    "type_ref_count": 13,
                },
                {
                    "class_names": ["a/Other.class"],
                    "primary": False,
                    "weight_estimate": 0,
                    "method_ref_count": 0,
                    "field_ref_count": 0,
                    "type_ref_count": 0,
                },
            ],
        )

    def test_empty_library_has_no_units(self) -> None:
        self.assertEqual(self._parse("\n", []), [])

    def test_malformed_rows_are_rejected_with_location(self) -> None:
        malformed = [
            "a/Foo\t1\t2\t3\t4",
            "a/Foo 1 2 3 4 a/Foo",
            "a/Foo\t-1\t2\t3\t4\ta/Foo",
            "a/Foo\t1\t-2\t3\t4\ta/Foo",
            "a/Foo\t1\t2\t-3\t4\ta/Foo",
            "a/Foo\t1\t2\t3\t-4\ta/Foo",
            "a/Foo\t1\t2.5\t3\t4\ta/Foo",
            "a/Foo\t1\t2\tthree\t4\ta/Foo",
            "a/Foo\t1\t2\t3\t\ta/Foo",
            "a/Foo\t1\t2\t3\t4\ta/Other",
            "a/Foo\t1\t2\t3\t4\ta/Foo\ta/Foo",
            "a/Foo.class\t1\t2\t3\t4\ta/Foo.class",
            "a/Foo\t1\t2\t3\t4\ta/Foo\tz/one.class",
            "a.Foo\t1\t2\t3\t4\ta.Foo",
            "a/Foo\t1\t2\t3\t4\ta/Foo\t",
        ]
        for row in malformed:
            with (
                self.subTest(row=row),
                self.assertRaisesRegex(ValueError, r"units.tsv:1"),
            ):
                self._parse(row + "\n")

    def test_member_cannot_belong_to_multiple_units(self) -> None:
        with self.assertRaisesRegex(ValueError, "multiple dex units"):
            self._parse(
                "a/Foo\t1\t2\t3\t4\ta/Foo\tz/one\n"
                "a/Other\t1\t2\t3\t4\ta/Other\tz/one\n",
                ["a/Foo", "a/Other", "z/one"],
            )

    def test_missing_and_unexpected_members_are_rejected(self) -> None:
        for classes, row, error in [
            (["a/Foo", "z/one"], "a/Foo\t1\t2\t3\t4\ta/Foo\n", "missing=\\['z/one'\\]"),
            (
                ["a/Foo"],
                "a/Foo\t1\t2\t3\t4\ta/Foo\tz/one\n",
                "unexpected=\\['z/one'\\]",
            ),
        ]:
            with (
                self.subTest(classes=classes),
                self.assertRaisesRegex(ValueError, error),
            ):
                self._parse(row, classes)


class FilterDexToolTest(unittest.TestCase):
    def _run_filter(self, libraries: list[dict[str, str]], patterns: str = ""):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            patterns_path = root / "patterns.txt"
            patterns_path.write_text(patterns)
            output_path = root / "filter.json"
            argv = [
                "filter_dex",
                "--primary-dex-patterns",
                str(patterns_path),
                "--output",
                str(output_path),
                "--dex-target-identifiers",
                *[f"lib{i}" for i in range(len(libraries))],
            ]
            for flag, key, default in [
                ("--class-names", "classes", ""),
                ("--weight-estimates", "weight", "123"),
                ("--ref-counts", "refs", "11 12 13"),
                ("--synthetic-contexts", "contexts", None),
                ("--dex-units", "units", None),
            ]:
                paths = []
                for i, library in enumerate(libraries):
                    contents = library.get(key, default)
                    if contents is not None:
                        path = root / f"{i}.{key}"
                        path.write_text(contents)
                        paths.append(str(path))
                if paths:
                    argv.extend([flag, *paths])
            with patch.object(sys, "argv", argv):
                main()
            return json.loads(output_path.read_text())

    def test_context_and_multiple_companions_are_primary_together(self) -> None:
        result = self._run_filter(
            [
                {
                    "classes": "z/one\na/Other\na/Foo\nz/two\n",
                    "contexts": "z/one a/Foo\nz/two z/one\n",
                    "units": (
                        "a/Foo\t100\t10\t20\t30\ta/Foo\tz/one\tz/two\n"
                        "a/Other\t23\t1\t2\t3\ta/Other\n"
                    ),
                }
            ],
            "^a/Foo^\n",
        )["lib0"]
        self.assertEqual(
            result["primary_dex_class_names"],
            ["z/one.class", "a/Foo.class", "z/two.class"],
        )
        self.assertEqual(result["secondary_dex_class_names"], ["a/Other.class"])
        self.assertTrue(result["dex_units"][0]["primary"])
        self.assertFalse(result["dex_units"][1]["primary"])
        self.assertEqual(result["weight_estimate"], "123")
        self.assertEqual(result["method_ref_count"], "11")
        self.assertEqual(result["field_ref_count"], "12")
        self.assertEqual(result["type_ref_count"], "13")

    def test_synthetic_only_pattern_moves_whole_unit(self) -> None:
        result = self._run_filter(
            [
                {
                    "classes": "a/Foo\nz/one\nz/two\n",
                    "contexts": "z/one a/Foo\nz/two a/Foo\n",
                    "units": "a/Foo\t123\t11\t12\t13\ta/Foo\tz/one\tz/two\n",
                }
            ],
            "^z/two^\n",
        )["lib0"]
        self.assertEqual(
            result["primary_dex_class_names"],
            ["a/Foo.class", "z/one.class", "z/two.class"],
        )
        self.assertEqual(result["secondary_dex_class_names"], [])
        self.assertTrue(result["dex_units"][0]["primary"])

    def test_member_context_rule_moves_whole_unit(self) -> None:
        result = self._run_filter(
            [
                {
                    "classes": "b/Holder\nz/one\nz/two\n",
                    "contexts": "z/one a/Entry\nz/two z/one\n",
                    "units": "b/Holder\t123\t11\t12\t13\tb/Holder\tz/one\tz/two\n",
                }
            ],
            "^a/Entry^\n",
        )["lib0"]
        self.assertEqual(
            result["primary_dex_class_names"],
            ["b/Holder.class", "z/one.class", "z/two.class"],
        )
        self.assertTrue(result["dex_units"][0]["primary"])

    def test_multiple_library_unit_inputs_stay_aligned(self) -> None:
        result = self._run_filter(
            [
                {"classes": "a/Foo\n", "units": "a/Foo\t1\t2\t3\t4\ta/Foo\n"},
                {"classes": "b/Bar\n", "units": "b/Bar\t5\t6\t7\t8\tb/Bar\n"},
            ],
            "^b/Bar^\n",
        )
        self.assertEqual(result["lib0"]["dex_units"][0]["weight_estimate"], 1)
        self.assertFalse(result["lib0"]["dex_units"][0]["primary"])
        self.assertEqual(result["lib1"]["dex_units"][0]["method_ref_count"], 6)
        self.assertTrue(result["lib1"]["dex_units"][0]["primary"])

    def test_mismatched_per_library_unit_inputs_are_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "same number of dex unit files"):
            self._run_filter(
                [
                    {"classes": "a/Foo\n", "units": "a/Foo\t1\t2\t3\t4\ta/Foo\n"},
                    {"classes": "b/Bar\n"},
                ]
            )

    def test_absent_unit_metadata_preserves_legacy_output(self) -> None:
        result = self._run_filter(
            [{"classes": "a/Foo\nz/one\nb/Bar\n", "contexts": "z/one a/Foo\n"}],
            "^a/Foo^\n",
        )["lib0"]
        self.assertEqual(
            result,
            {
                "primary_dex_class_names": ["a/Foo.class", "z/one.class"],
                "secondary_dex_class_names": ["b/Bar.class"],
                "weight_estimate": "123",
                "method_ref_count": "11",
                "field_ref_count": "12",
                "type_ref_count": "13",
            },
        )

    def test_empty_unit_metadata_is_not_legacy_fallback(self) -> None:
        self.assertEqual(self._run_filter([{"units": ""}])["lib0"]["dex_units"], [])
        with self.assertRaisesRegex(ValueError, "missing="):
            self._run_filter([{"classes": "a/Foo\n", "units": ""}])
