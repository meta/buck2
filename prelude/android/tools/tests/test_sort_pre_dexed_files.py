# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Tests for sort_pre_dexed_files.py — the Python tool that replaces the
_sort_pre_dexed_files Starlark function in dex_rules.bzl.

The tool reads filter_dex JSON outputs and a lib metadata JSON, then sorts
pre-dexed libraries into primary/secondary dex groups per module, producing
a dex_plan.json consumed by the merge_pre_dexed_libs dynamic lambda.

Test structure:
  - SortPreDexedFilesTest: unit tests for the core sorting logic
  - ParseModuleGraphTest: unit tests for module graph file parsing
  - EndToEndToolTest: integration tests running the tool as a subprocess
"""

import json
import os
import subprocess
import tempfile
import unittest

from android.tools.filter_dex import _parse_ref_counts
from android.tools.sort_pre_dexed_files import (
    _flatten_groups,
    _parse_module_graph,
    _sort_pre_dexed_files,
)
from android.tools.tests.generate_mobileconfig_fixture import fixture_expectations

# Path to the tool script, relative to the link-tree layout
_TOOL_MODULE_PATH = os.path.join(
    os.path.dirname(__file__), "..", "sort_pre_dexed_files.py"
)

_DEFAULT_WEIGHT_LIMIT = 12 * 1024 * 1024  # 12 MiB, matches Buck default

# Raw pre_dex counts and expectations are captured together by
# generate_mobileconfig_fixture.py. Its recipe pins the build's weight estimate,
# fb4a's 5 MiB limit and independently reviewed group boundaries.
_MOBILECONFIG_FIXTURE_PATH = os.path.join(
    os.path.dirname(__file__), "testdata", "mobileconfig_definition_lib_ref_count"
)

_DEFAULT_REF_COUNTS = {
    "method_ref_count": "0",
    "field_ref_count": "0",
    "type_ref_count": "0",
}


def _add_default_ref_counts(filter_data):
    """Add default zero ref counts to any filter_data entries missing them."""
    for info in filter_data.values():
        for key, default_val in _DEFAULT_REF_COUNTS.items():
            if key not in info:
                info[key] = default_val
    return filter_data


def _sort(filter_data, lib_metadata, target_to_module=None, **kwargs):
    """Convenience wrapper with defaults for _sort_pre_dexed_files."""
    _add_default_ref_counts(filter_data)
    return _sort_pre_dexed_files(
        filter_data,
        lib_metadata,
        target_to_module or {},
        weight_limit=kwargs.get("weight_limit", _DEFAULT_WEIGHT_LIMIT),
        enable_bootstrap_dexes=kwargs.get("enable_bootstrap_dexes", False),
    )


class MobileConfigFixtureTest(unittest.TestCase):
    def test_checked_in_expectations_are_reproducible(self):
        with open(_MOBILECONFIG_FIXTURE_PATH + ".txt") as f:
            ref_counts = f.read()
        with open(_MOBILECONFIG_FIXTURE_PATH + ".json") as f:
            fixture = json.load(f)
        self.assertEqual(
            fixture_expectations(
                ref_counts,
                fixture["weight_estimate"],
                [len(group["class_names"]) for group in fixture["expected_groups"]],
                fixture["source_revision"],
            ),
            fixture,
        )

    def test_expected_counts_come_from_raw_class_rows(self):
        fixture = fixture_expectations(
            "2 30 4\n1 20 3 a/Z\n1 10 2 a/A\n", 100, [2], "test"
        )
        self.assertEqual(
            fixture["expected_groups"],
            [{"class_names": ["a/A.class", "a/Z.class"], "ref_counts": [2, 30, 5]}],
        )

    def test_group_sizes_must_cover_every_class(self):
        for sizes in ([0, 1], [], [2]):
            with self.subTest(sizes=sizes), self.assertRaises(ValueError):
                fixture_expectations("1 2 3\n1 2 3 a/A\n", 100, sizes, "test")


class SortPreDexedFilesTest(unittest.TestCase):
    """Unit tests for _sort_pre_dexed_files — the core sorting logic.

    The function takes:
      - filter_data: merged filter_dex JSON (identifier -> class names + weight)
      - lib_metadata: identifier -> owner target label (controls iteration order)
      - target_to_module: target -> module name (from module graph)
      - weight_limit: max bytes per dex group before spilling
      - enable_bootstrap_dexes: whether to apply weight limit to primary dex too

    It returns: module -> {primary_groups: [...], secondary_groups: [...]}
    where each group is a list of {id, class_names} entries.
    """

    # --- Basic grouping ---

    def test_single_lib_primary_and_secondary(self):
        """A single lib with both primary and secondary classes produces one
        group in each category under the root module ("dex")."""
        filter_data = {
            "lib1": {
                "primary_dex_class_names": ["com.example.Main"],
                "secondary_dex_class_names": ["com.example.Helper"],
                "weight_estimate": "1000",
            },
        }
        result = _sort(filter_data, {"lib1": "//apps:lib1"})

        self.assertIn("dex", result)
        self.assertEqual(len(result["dex"]["primary_groups"]), 1)
        self.assertEqual(len(result["dex"]["secondary_groups"]), 1)
        self.assertEqual(
            result["dex"]["primary_groups"][0][0]["class_names"], ["com.example.Main"]
        )
        self.assertEqual(
            result["dex"]["secondary_groups"][0][0]["class_names"],
            ["com.example.Helper"],
        )

    def test_multiple_libs_grouped_under_weight_limit(self):
        """Multiple small libs whose combined weight is under the limit should
        land in the same secondary dex group."""
        filter_data = {
            "lib1": {
                "primary_dex_class_names": [],
                "secondary_dex_class_names": ["com.A"],
                "weight_estimate": "100",
            },
            "lib2": {
                "primary_dex_class_names": [],
                "secondary_dex_class_names": ["com.B"],
                "weight_estimate": "100",
            },
        }
        result = _sort(filter_data, {"lib1": "//apps:lib1", "lib2": "//apps:lib2"})

        self.assertEqual(len(result["dex"]["secondary_groups"]), 1)
        self.assertEqual(len(result["dex"]["secondary_groups"][0]), 2)

    def test_empty_class_names_produce_no_groups(self):
        """Libs with empty primary and secondary class names should not create
        any dex groups — they are effectively no-ops."""
        filter_data = {
            "lib1": {
                "primary_dex_class_names": [],
                "secondary_dex_class_names": [],
                "weight_estimate": "0",
            },
        }
        result = _sort(filter_data, {"lib1": "//apps:lib1"})

        self.assertEqual(len(result.get("dex", {}).get("primary_groups", [])), 0)
        self.assertEqual(len(result.get("dex", {}).get("secondary_groups", [])), 0)

    # --- Weight-based splitting ---

    def test_weight_limit_splits_secondary_groups(self):
        """When cumulative weight of libs exceeds the limit, subsequent libs
        should spill into a new secondary dex group."""
        filter_data = {
            "lib1": {
                "primary_dex_class_names": [],
                "secondary_dex_class_names": ["com.A"],
                "weight_estimate": "600",
            },
            "lib2": {
                "primary_dex_class_names": [],
                "secondary_dex_class_names": ["com.B"],
                "weight_estimate": "600",
            },
        }
        result = _sort(
            filter_data,
            {"lib1": "//apps:lib1", "lib2": "//apps:lib2"},
            weight_limit=1000,
        )

        # lib1 (600) + lib2 (600) = 1200 > 1000 limit
        self.assertEqual(len(result["dex"]["secondary_groups"]), 2)

    def test_bootstrap_dexes_applies_weight_limit_to_primary(self):
        """With enable_bootstrap_dexes=True, the weight limit also applies to
        primary dex groups, splitting them when exceeded."""
        filter_data = {
            "lib1": {
                "primary_dex_class_names": ["com.A"],
                "secondary_dex_class_names": [],
                "weight_estimate": "600",
            },
            "lib2": {
                "primary_dex_class_names": ["com.B"],
                "secondary_dex_class_names": [],
                "weight_estimate": "600",
            },
        }
        result = _sort(
            filter_data,
            {"lib1": "//apps:lib1", "lib2": "//apps:lib2"},
            weight_limit=1000,
            enable_bootstrap_dexes=True,
        )

        # 600 + 600 = 1200 > 1000 -> splits into 2 primary groups
        self.assertEqual(len(result["dex"]["primary_groups"]), 2)

    def test_large_lib_classes_chunked_across_groups(self):
        """A single lib whose weight exceeds the limit should have its classes
        chunked across multiple groups. All classes must be preserved."""
        classes = ["com.Class{}".format(i) for i in range(10)]
        filter_data = {
            "lib1": {
                "primary_dex_class_names": [],
                "secondary_dex_class_names": classes,
                "weight_estimate": "2500",
            },
        }
        result = _sort(filter_data, {"lib1": "//apps:lib1"}, weight_limit=1000)

        # 2500 > 1000 -> classes chunked across multiple groups
        self.assertGreater(len(result["dex"]["secondary_groups"]), 1)
        all_classes = []
        for group in result["dex"]["secondary_groups"]:
            for entry in group:
                all_classes.extend(entry["class_names"])
        self.assertCountEqual(all_classes, classes)

    # --- Module assignment ---

    def test_non_root_module_primary_classes_moved_to_secondary(self):
        """Primary dex classes assigned to a non-root module are relocated to
        that module's secondary dex. Only the root module ("dex") may have
        primary dex classes."""
        filter_data = {
            "lib1": {
                "primary_dex_class_names": ["com.feature.Primary"],
                "secondary_dex_class_names": ["com.feature.Secondary"],
                "weight_estimate": "1000",
            },
        }
        result = _sort(
            filter_data,
            {"lib1": "//apps:lib1"},
            target_to_module={"//apps:lib1": "feature_module"},
        )

        self.assertIn("feature_module", result)
        self.assertEqual(len(result["feature_module"]["primary_groups"]), 0)
        all_secondary_classes = []
        for group in result["feature_module"]["secondary_groups"]:
            for entry in group:
                all_secondary_classes.extend(entry["class_names"])
        self.assertIn("com.feature.Primary", all_secondary_classes)
        self.assertIn("com.feature.Secondary", all_secondary_classes)

    def test_multiple_modules_separated(self):
        """Libs assigned to different modules via the target-to-module mapping
        should be placed in separate module entries."""
        filter_data = {
            "lib1": {
                "primary_dex_class_names": ["com.main.Main"],
                "secondary_dex_class_names": [],
                "weight_estimate": "1000",
            },
            "lib2": {
                "primary_dex_class_names": [],
                "secondary_dex_class_names": ["com.feature.Impl"],
                "weight_estimate": "1000",
            },
        }
        result = _sort(
            filter_data,
            {"lib1": "//apps:lib1", "lib2": "//apps:lib2"},
            target_to_module={"//apps:lib1": "dex", "//apps:lib2": "feature1"},
        )

        self.assertIn("dex", result)
        self.assertIn("feature1", result)
        self.assertEqual(len(result["dex"]["primary_groups"]), 1)
        self.assertEqual(len(result["feature1"]["secondary_groups"]), 1)

    def test_unmapped_target_defaults_to_root_module(self):
        """Libs whose owner target is not in the target-to-module mapping
        should default to the root module ("dex")."""
        filter_data = {
            "lib1": {
                "primary_dex_class_names": [],
                "secondary_dex_class_names": ["com.Unknown"],
                "weight_estimate": "100",
            },
        }
        # target_to_module has an entry for a different target, not lib1's owner
        result = _sort(
            filter_data,
            {"lib1": "//apps:lib1"},
            target_to_module={"//other:target": "feature1"},
        )

        self.assertIn("dex", result)
        self.assertNotIn("feature1", result)
        self.assertEqual(len(result["dex"]["secondary_groups"]), 1)

    # --- Ref-count-based splitting ---

    def test_ref_count_splits_secondary_groups(self):
        """When cumulative method ref count exceeds DEX_REF_LIMIT (65536),
        subsequent libs should spill into a new secondary dex group, even
        if weight is well under the weight limit."""
        filter_data = {
            "lib1": {
                "primary_dex_class_names": [],
                "secondary_dex_class_names": ["com.A"],
                "weight_estimate": "100",
                "method_ref_count": "40000",
                "field_ref_count": "0",
                "type_ref_count": "0",
            },
            "lib2": {
                "primary_dex_class_names": [],
                "secondary_dex_class_names": ["com.B"],
                "weight_estimate": "100",
                "method_ref_count": "40000",
                "field_ref_count": "0",
                "type_ref_count": "0",
            },
        }
        result = _sort(
            filter_data,
            {"lib1": "//apps:lib1", "lib2": "//apps:lib2"},
            weight_limit=_DEFAULT_WEIGHT_LIMIT,
        )

        # 40000 + 40000 = 80000 > 65536 -> splits into 2 secondary groups
        self.assertEqual(len(result["dex"]["secondary_groups"]), 2)

    def test_high_ref_counts_do_not_split_primary_without_bootstrap(self):
        """Without enable_bootstrap_dexes, primary dex groups must NOT be
        split even when ref counts exceed the limit. Splitting primary
        without the bootstrap classloader causes ClassNotFoundException."""
        filter_data = {
            "lib1": {
                "primary_dex_class_names": ["com.A"],
                "secondary_dex_class_names": [],
                "weight_estimate": "100",
                "method_ref_count": "40000",
                "field_ref_count": "0",
                "type_ref_count": "0",
            },
            "lib2": {
                "primary_dex_class_names": ["com.B"],
                "secondary_dex_class_names": [],
                "weight_estimate": "100",
                "method_ref_count": "40000",
                "field_ref_count": "0",
                "type_ref_count": "0",
            },
        }
        result = _sort(
            filter_data,
            {"lib1": "//apps:lib1", "lib2": "//apps:lib2"},
            weight_limit=_DEFAULT_WEIGHT_LIMIT,
            enable_bootstrap_dexes=False,
        )

        # Without bootstrap, primary must stay in a single group
        self.assertEqual(len(result["dex"]["primary_groups"]), 1)

    def test_large_lib_chunked_by_ref_count(self):
        """A single lib whose method ref count exceeds DEX_REF_LIMIT should
        have its classes chunked across multiple groups, similar to
        weight-based chunking."""
        classes = ["com.Class{}".format(i) for i in range(10)]
        filter_data = {
            "lib1": {
                "primary_dex_class_names": [],
                "secondary_dex_class_names": classes,
                "weight_estimate": "100",
                "method_ref_count": "200000",
                "field_ref_count": "0",
                "type_ref_count": "0",
            },
        }
        result = _sort(
            filter_data, {"lib1": "//apps:lib1"}, weight_limit=_DEFAULT_WEIGHT_LIMIT
        )

        # 200000 > 65536 -> classes chunked across multiple groups
        self.assertGreater(len(result["dex"]["secondary_groups"]), 1)
        all_classes = []
        for group in result["dex"]["secondary_groups"]:
            for entry in group:
                all_classes.extend(entry["class_names"])
        self.assertCountEqual(all_classes, classes)

    def test_real_mobileconfig_ref_counts_split_only_the_overflowing_chunk(self):
        """The pinned build forms a fitting first run and an overflowing tail.
        Only the tail is split, matching the fixture's reviewed expectations."""
        with open(_MOBILECONFIG_FIXTURE_PATH + ".json") as f:
            fixture = json.load(f)
        method_ref_count, field_ref_count, type_ref_count, class_ref_counts = (
            _parse_ref_counts(_MOBILECONFIG_FIXTURE_PATH + ".txt")
        )
        # filter_dex lists classes in pre-dex class name order, with a .class suffix.
        classes = sorted(class_ref_counts, key=lambda name: name.removesuffix(".class"))
        filter_data = {
            "mc": {
                "primary_dex_class_names": [],
                "secondary_dex_class_names": classes,
                "weight_estimate": str(fixture["weight_estimate"]),
                "method_ref_count": method_ref_count,
                "field_ref_count": field_ref_count,
                "type_ref_count": type_ref_count,
                "class_ref_counts": class_ref_counts,
            },
        }
        # fb4a's 5 MiB secondary limit comes from
        # tools/buckconfigs/fbsource-mobile-common.bcfg, not Buck's 12 MiB fallback.
        result = _sort(
            filter_data,
            {"mc": "//apps:mc"},
            weight_limit=fixture["secondary_dex_weight_limit"],
        )

        groups = [
            [name for entry in group for name in entry["class_names"]]
            for group in result["dex"]["secondary_groups"]
        ]
        self.assertEqual(
            groups, [group["class_names"] for group in fixture["expected_groups"]]
        )
        self.assertEqual(
            [
                [sum(class_ref_counts[name][i] for name in group) for i in range(3)]
                for group in groups
            ],
            [group["ref_counts"] for group in fixture["expected_groups"]],
        )
        self.assertCountEqual([name for group in groups for name in group], classes)

    def test_chunks_that_fit_by_class_ref_counts_are_kept(self):
        """A lib just over the method limit with refs spread evenly. Its class count chunks
        fit, so the per-class counts do not change them."""
        classes = [
            "com/foo/Test{}.class".format(c) for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
        ]

        def plan(class_ref_counts):
            filter_data = {
                "lib": {
                    "primary_dex_class_names": [],
                    "secondary_dex_class_names": classes,
                    "weight_estimate": "5482334",
                    "method_ref_count": "66354",
                    "field_ref_count": "0",
                    "type_ref_count": "30",
                    "class_ref_counts": class_ref_counts,
                },
            }
            result = _sort(filter_data, {"lib": "//apps:lib"}, weight_limit=5000000)
            return [
                [name for entry in group for name in entry["class_names"]]
                for group in result["dex"]["secondary_groups"]
            ]

        groups = plan({name: [2553, 0, 3] for name in classes})
        self.assertEqual(groups, [classes[:23], classes[23:]])
        self.assertEqual(groups, plan({}))

    def test_canary_types_are_left_room_in_each_secondary_chunk(self):
        """Each secondary dex also gets a canary with up to two types, so a secondary chunk can
        hold 65534 types of its own. Primary and bootstrap dexes get no canary."""
        classes = ["com/A{}.class".format(i) for i in range(4)]

        def plan(third_class_types, groups):
            types = [30000, 30000, third_class_types, 1]
            in_primary = groups == "primary_groups"
            filter_data = {
                "lib": {
                    "primary_dex_class_names": classes if in_primary else [],
                    "secondary_dex_class_names": [] if in_primary else classes,
                    "weight_estimate": "100",
                    "method_ref_count": "0",
                    "field_ref_count": "70000",
                    "type_ref_count": "100",
                    "class_ref_counts": {
                        name: [0, 17500, t] for name, t in zip(classes, types)
                    },
                },
            }
            result = _sort(
                filter_data, {"lib": "//apps:lib"}, enable_bootstrap_dexes=True
            )
            return [
                [name for entry in group for name in entry["class_names"]]
                for group in result["dex"][groups]
            ]

        split = [classes[:2], classes[2:3], classes[3:]]
        self.assertEqual(plan(5534, "secondary_groups"), [classes[:3], classes[3:]])
        self.assertEqual(plan(5535, "secondary_groups"), split)
        self.assertEqual(plan(5536, "primary_groups"), [classes[:3], classes[3:]])
        self.assertEqual(plan(5537, "primary_groups"), split)

    def test_class_count_chunks_are_kept_without_counts_for_every_class(self):
        """Per-class counts that miss a class are not used."""
        classes = ["com/A{}.class".format(i) for i in range(4)]
        filter_data = {
            "lib": {
                "primary_dex_class_names": [],
                "secondary_dex_class_names": classes,
                "weight_estimate": "100",
                "method_ref_count": "0",
                "field_ref_count": "70000",
                "type_ref_count": "10",
                "class_ref_counts": {name: [0, 40000, 1] for name in classes[:3]},
            },
        }
        result = _sort(filter_data, {"lib": "//apps:lib"})

        groups = [
            [name for entry in group for name in entry["class_names"]]
            for group in result["dex"]["secondary_groups"]
        ]
        self.assertEqual(groups, [classes[:3], classes[3:]])

    # --- Ordering and skipping ---

    def test_iteration_order_follows_lib_metadata_keys(self):
        """Libs should be processed in lib_metadata key insertion order, not
        filter_data order. This is critical for deterministic dex assignment."""
        filter_data = {
            "lib_b": {
                "primary_dex_class_names": [],
                "secondary_dex_class_names": ["com.B"],
                "weight_estimate": "600",
            },
            "lib_a": {
                "primary_dex_class_names": [],
                "secondary_dex_class_names": ["com.A"],
                "weight_estimate": "600",
            },
        }
        # lib_a first in metadata -> processed first -> gets first group
        lib_metadata = {"lib_a": "//apps:lib_a", "lib_b": "//apps:lib_b"}
        result = _sort(filter_data, lib_metadata, weight_limit=1000)

        self.assertEqual(len(result["dex"]["secondary_groups"]), 2)
        self.assertEqual(
            result["dex"]["secondary_groups"][0][0]["class_names"], ["com.A"]
        )
        self.assertEqual(
            result["dex"]["secondary_groups"][1][0]["class_names"], ["com.B"]
        )

    def test_lib_in_metadata_but_not_filter_data_skipped(self):
        """Libs present in lib_metadata but absent from filter_data (e.g.,
        libs with no dex output) should be silently skipped."""
        filter_data = {
            "lib1": {
                "primary_dex_class_names": [],
                "secondary_dex_class_names": ["com.A"],
                "weight_estimate": "100",
            },
        }
        # lib2 in metadata but not in filter_data
        result = _sort(filter_data, {"lib1": "//apps:lib1", "lib2": "//apps:lib2"})

        self.assertEqual(len(result["dex"]["secondary_groups"]), 1)
        self.assertEqual(result["dex"]["secondary_groups"][0][0]["id"], "lib1")


class FlattenGroupsTest(unittest.TestCase):
    """Unit tests for _flatten_groups — converts the internal per-entry
    representation [{id, class_names}, ...] into the plan JSON format
    {lib_ids: [...], class_names: [...]} per group."""

    def test_single_group_single_entry(self):
        groups = [[{"id": "lib1", "class_names": ["com.A", "com.B"]}]]
        result = _flatten_groups(groups)
        self.assertEqual(len(result), 1)
        self.assertEqual(result[0]["lib_ids"], ["lib1"])
        self.assertEqual(result[0]["class_names"], ["com.A", "com.B"])

    def test_single_group_multiple_entries(self):
        """Multiple entries in one group should have their lib_ids and
        class_names concatenated in order."""
        groups = [
            [
                {"id": "lib1", "class_names": ["com.A"]},
                {"id": "lib2", "class_names": ["com.B", "com.C"]},
            ]
        ]
        result = _flatten_groups(groups)
        self.assertEqual(result[0]["lib_ids"], ["lib1", "lib2"])
        self.assertEqual(result[0]["class_names"], ["com.A", "com.B", "com.C"])

    def test_multiple_groups(self):
        groups = [
            [{"id": "lib1", "class_names": ["com.A"]}],
            [{"id": "lib2", "class_names": ["com.B"]}],
        ]
        result = _flatten_groups(groups)
        self.assertEqual(len(result), 2)
        self.assertEqual(result[0]["lib_ids"], ["lib1"])
        self.assertEqual(result[1]["lib_ids"], ["lib2"])

    def test_empty_groups(self):
        self.assertEqual(_flatten_groups([]), [])


class ParseModuleGraphTest(unittest.TestCase):
    """Unit tests for _parse_module_graph.

    The module graph file format is:
      Line 0: <module_count>
      Lines 1..module_count: <module_name> <canary_class> <dep1> <dep2> ...
      Remaining lines: <target> <module>
      Last line: empty (trailing newline)
    """

    def _write_module_graph(self, content):
        """Write content to a temp file and return its path."""
        f = tempfile.NamedTemporaryFile(mode="w", suffix=".txt", delete=False)
        f.write(content)
        f.close()
        self.addCleanup(os.unlink, f.name)
        return f.name

    def test_none_returns_empty_dicts(self):
        target_to_module, module_metadata = _parse_module_graph(None)
        self.assertEqual(target_to_module, {})
        self.assertEqual(module_metadata, {})

    def test_simple_graph(self):
        """Parse a graph with 2 modules and 2 target mappings."""
        path = self._write_module_graph(
            "2\n"
            "dex com/facebook/CanaryClass dex\n"
            "feature1 com/facebook/feature1/Canary dex\n"
            "//apps:lib1 dex\n"
            "//apps:lib2 feature1\n"
        )
        target_to_module, module_metadata = _parse_module_graph(path)

        self.assertEqual(
            target_to_module,
            {
                "//apps:lib1": "dex",
                "//apps:lib2": "feature1",
            },
        )
        self.assertEqual(
            module_metadata["dex"]["canary_class_name"], "com/facebook/CanaryClass"
        )
        self.assertEqual(module_metadata["dex"]["module_deps"], ["dex"])
        self.assertEqual(module_metadata["feature1"]["module_deps"], ["dex"])

    def test_modules_only_no_target_mappings(self):
        """A module graph with modules defined but no target-to-module mappings
        should produce empty target_to_module but populated module_metadata."""
        path = self._write_module_graph("1\ndex com/CanaryClass dex\n")
        target_to_module, module_metadata = _parse_module_graph(path)

        self.assertEqual(target_to_module, {})
        self.assertIn("dex", module_metadata)

    def test_trailing_newline_does_not_drop_last_mapping(self):
        """The file format ends with a trailing newline. The [:-1] slice in
        _parse_module_graph strips this empty last line. Verify the last
        real target mapping is preserved."""
        path = self._write_module_graph(
            "1\ndex com/CanaryClass dex\n//apps:last_target dex\n"
        )
        target_to_module, _ = _parse_module_graph(path)

        self.assertIn("//apps:last_target", target_to_module)
        self.assertEqual(target_to_module["//apps:last_target"], "dex")


class EndToEndToolTest(unittest.TestCase):
    """Integration tests running sort_pre_dexed_files.py as a subprocess.

    These verify the full pipeline: arg parsing, file I/O, sorting, and JSON
    output. They complement the unit tests which test internal functions directly.
    """

    def _run_tool(
        self,
        tmpdir,
        filter_data_files,
        lib_metadata,
        weight_limit=_DEFAULT_WEIGHT_LIMIT,
        module_graph=None,
        enable_bootstrap_dexes=False,
    ):
        """Write inputs, run the tool, return parsed plan JSON."""
        # Write filter_dex outputs
        filter_paths = []
        for i, data in enumerate(filter_data_files):
            _add_default_ref_counts(data)
            path = os.path.join(tmpdir, "filter_output_{}.json".format(i))
            with open(path, "w") as f:
                json.dump(data, f)
            filter_paths.append(path)

        # Write lib metadata
        metadata_path = os.path.join(tmpdir, "lib_metadata.json")
        with open(metadata_path, "w") as f:
            json.dump(lib_metadata, f)

        # Build command
        output_path = os.path.join(tmpdir, "plan.json")
        cmd = [
            "python3",
            _TOOL_MODULE_PATH,
            "--filter-dex-outputs",
            *filter_paths,
            "--lib-metadata",
            metadata_path,
            "--weight-limit",
            str(weight_limit),
            "--output",
            output_path,
        ]
        if module_graph:
            graph_path = os.path.join(tmpdir, "module_graph.txt")
            with open(graph_path, "w") as f:
                f.write(module_graph)
            cmd.extend(["--module-graph", graph_path])
        if enable_bootstrap_dexes:
            cmd.append("--enable-bootstrap-dexes")

        result = subprocess.run(cmd, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, "Tool failed: {}".format(result.stderr))

        with open(output_path) as f:
            return json.load(f)

    def test_basic_plan_output(self):
        """Two libs with primary and secondary classes produce a valid plan
        with correct lib_ids and class_names."""
        with tempfile.TemporaryDirectory() as tmpdir:
            plan = self._run_tool(
                tmpdir,
                filter_data_files=[
                    {
                        "lib1": {
                            "primary_dex_class_names": ["com.Main"],
                            "secondary_dex_class_names": ["com.Helper"],
                            "weight_estimate": "1000",
                        },
                        "lib2": {
                            "primary_dex_class_names": [],
                            "secondary_dex_class_names": ["com.Utils"],
                            "weight_estimate": "2000",
                        },
                    }
                ],
                lib_metadata={"lib1": "//apps:lib1", "lib2": "//apps:lib2"},
            )

            self.assertEqual(len(plan["modules"]), 1)
            module = plan["modules"][0]
            self.assertEqual(module["module"], "dex")
            self.assertEqual(module["primary_groups"][0]["lib_ids"], ["lib1"])
            self.assertEqual(module["primary_groups"][0]["class_names"], ["com.Main"])
            self.assertEqual(len(module["secondary_groups"]), 1)
            self.assertIn("lib1", module["secondary_groups"][0]["lib_ids"])
            self.assertIn("lib2", module["secondary_groups"][0]["lib_ids"])

    def test_with_module_graph(self):
        """Libs are correctly assigned to different modules when a module
        graph file is provided."""
        with tempfile.TemporaryDirectory() as tmpdir:
            plan = self._run_tool(
                tmpdir,
                filter_data_files=[
                    {
                        "lib1": {
                            "primary_dex_class_names": ["com.Main"],
                            "secondary_dex_class_names": [],
                            "weight_estimate": "1000",
                        },
                        "lib2": {
                            "primary_dex_class_names": [],
                            "secondary_dex_class_names": ["com.Feature"],
                            "weight_estimate": "1000",
                        },
                    }
                ],
                lib_metadata={"lib1": "//apps:lib1", "lib2": "//apps:lib2"},
                module_graph=(
                    "2\n"
                    "dex com/CanaryClass dex\n"
                    "feature1 com/feature1/Canary dex\n"
                    "//apps:lib1 dex\n"
                    "//apps:lib2 feature1\n"
                ),
            )

            modules_by_name = {m["module"]: m for m in plan["modules"]}
            self.assertIn("dex", modules_by_name)
            self.assertIn("feature1", modules_by_name)
            self.assertEqual(
                modules_by_name["dex"]["primary_groups"][0]["lib_ids"], ["lib1"]
            )
            self.assertEqual(
                modules_by_name["feature1"]["secondary_groups"][0]["lib_ids"], ["lib2"]
            )
            self.assertIn("feature1", plan["module_metadata"])

    def test_multiple_filter_dex_batches(self):
        """The tool should merge multiple --filter-dex-outputs files. Each
        file represents a batch of libs processed by filter_dex in parallel."""
        with tempfile.TemporaryDirectory() as tmpdir:
            batch1 = {
                "lib1": {
                    "primary_dex_class_names": ["com.A"],
                    "secondary_dex_class_names": [],
                    "weight_estimate": "100",
                },
            }
            batch2 = {
                "lib2": {
                    "primary_dex_class_names": [],
                    "secondary_dex_class_names": ["com.B"],
                    "weight_estimate": "100",
                },
            }
            plan = self._run_tool(
                tmpdir,
                filter_data_files=[batch1, batch2],
                lib_metadata={"lib1": "//apps:lib1", "lib2": "//apps:lib2"},
            )

            module = plan["modules"][0]
            # lib1 in primary, lib2 in secondary — both batches merged
            self.assertEqual(module["primary_groups"][0]["lib_ids"], ["lib1"])
            self.assertEqual(module["secondary_groups"][0]["lib_ids"], ["lib2"])

    def test_bootstrap_dexes_cli_flag(self):
        """The --enable-bootstrap-dexes flag should apply weight limit to
        primary dex groups, splitting them when exceeded."""
        with tempfile.TemporaryDirectory() as tmpdir:
            plan = self._run_tool(
                tmpdir,
                filter_data_files=[
                    {
                        "lib1": {
                            "primary_dex_class_names": ["com.A"],
                            "secondary_dex_class_names": [],
                            "weight_estimate": "600",
                        },
                        "lib2": {
                            "primary_dex_class_names": ["com.B"],
                            "secondary_dex_class_names": [],
                            "weight_estimate": "600",
                        },
                    }
                ],
                lib_metadata={"lib1": "//apps:lib1", "lib2": "//apps:lib2"},
                weight_limit=1000,
                enable_bootstrap_dexes=True,
            )

            module = plan["modules"][0]
            # 600 + 600 > 1000 -> 2 primary groups
            self.assertEqual(len(module["primary_groups"]), 2)
