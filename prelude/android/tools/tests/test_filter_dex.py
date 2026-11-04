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

import os
import tempfile
import unittest

from android.tools.filter_dex import (
    _belongs_in_primary_dex,
    _parse_ref_counts,
    _parse_synthetic_contexts,
    _resolve_synthesizing_context,
    ClassNameFilter,
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
