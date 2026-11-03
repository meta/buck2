#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import contextlib
import json
import os
import tempfile
import unittest
from unittest import mock

from cxx.dist_lto.tools import dist_lto_planner_gnu as planner


class DistLtoPlannerGnuTest(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root = self.temp_dir.name
        self.index_dir = os.path.join(self.root, "index")
        self.meta_path = os.path.join(self.root, "meta.json")
        self.link_plan_path = os.path.join(self.root, "link-plan.json")
        self.final_link_index_path = os.path.join(self.root, "final-link-index")
        self.argsfile_path = os.path.join(self.root, "thinlto_index_argsfile")
        os.makedirs(self.index_dir)
        self._write(self.argsfile_path, "")

    def tearDown(self):
        self.temp_dir.cleanup()

    def _write(self, path, contents):
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as outfile:
            outfile.write(contents)

    def _read_json(self, path):
        with open(path) as infile:
            return json.load(infile)

    def _read_lines(self, path):
        with open(path) as infile:
            return [line for line in infile.read().splitlines() if line]

    def _indexed_path(self, path):
        return os.path.join(self.index_dir, path)

    def _emit_bitcode_outputs(self, path, imports=()):
        self._write(self._indexed_path(path) + ".imports", "\n".join(imports))
        self._write(self._indexed_path(path) + ".thinlto.bc", "thinlto index")

    def _direct_linkable(self, name, idx):
        path = "objects/{}.o".format(name)
        output = os.path.join(self.root, "direct", name + ".thinlto.bc")
        plan = os.path.join(self.root, "direct", name + ".opt.plan")
        return (
            {
                "idx": idx,
                "output": output,
                "path": path,
                "plan_output": plan,
                "type": "bitcode",
            },
            path,
            output,
            plan,
        )

    def _archive_linkable(self, names, link_whole=False):
        # The planner resolves archive member paths relative to the archive's
        # base dir, so every archive path is relative to the planner's cwd.
        relative_base_dir = os.path.join("thin_lto_prepare", "lib")
        paths = [
            os.path.join(relative_base_dir, "objects", name + ".o") for name in names
        ]
        plan = os.path.join(relative_base_dir, "plan.json")
        indexes = os.path.join(relative_base_dir, "indexes")
        return (
            {
                "archive_idx": 0,
                "archive_index_dir": indexes,
                "archive_name": "lib",
                "archive_opt_objects_dir": os.path.join(
                    relative_base_dir, "opt_objects"
                ),
                "archive_plan": plan,
                "link_whole": link_whole,
                "objects": [{"path": path} for path in paths],
                "prepend": False,
                "type": "archive",
            },
            paths,
            os.path.join(self.root, plan),
            os.path.join(self.root, indexes),
        )

    def _run_planner(self, linkables, index_paths, full_index_lines):
        self._write(
            self.meta_path,
            json.dumps(
                [
                    {
                        "linkables": linkables,
                        "post_flags": [],
                        "pre_flags": [],
                    }
                ]
            ),
        )
        self._write(
            os.path.join(self.index_dir, "index"),
            "\n".join(self._indexed_path(path) for path in index_paths),
        )
        self._write(
            os.path.join(self.index_dir, "index.full"),
            "\n".join(full_index_lines),
        )

        argv = [
            "dist_lto_planner_gnu.py",
            "--meta",
            self.meta_path,
            "--index",
            self.index_dir,
            "--link-plan",
            self.link_plan_path,
            "--final-link-index",
            self.final_link_index_path,
            "--",
            "fake-linker",
            "@" + self.argsfile_path,
        ]
        with (
            contextlib.chdir(self.root),
            mock.patch.object(planner, "_enable_core_dumps"),
            mock.patch.object(planner.subprocess, "check_call") as check_call,
        ):
            self.assertIsNone(planner.main(argv))
        check_call.assert_called_once_with(["fake-linker", "@" + self.argsfile_path])

    def test_direct_selected_skipped_empty_and_non_lto(self):
        selected, selected_path, selected_output, selected_plan = self._direct_linkable(
            "selected", 0
        )
        skipped, skipped_path, skipped_output, skipped_plan = self._direct_linkable(
            "skipped", 1
        )
        empty, empty_path, empty_output, empty_plan = self._direct_linkable(
            "selected_empty", 2
        )
        native, native_path, _, native_plan = self._direct_linkable("native", 3)
        self._emit_bitcode_outputs(selected_path, [empty_path])
        self._emit_bitcode_outputs(skipped_path)
        self._emit_bitcode_outputs(empty_path)

        self._run_planner(
            [selected, skipped, empty, native],
            [selected_path, empty_path],
            [
                self._indexed_path(selected_path),
                self._indexed_path(empty_path),
                native_path,
            ],
        )

        selected_json = self._read_json(selected_plan)
        self.assertTrue(selected_json["is_bc"])
        self.assertFalse(selected_json["skip_backend"])
        self.assertEqual([2], selected_json["imports"])

        skipped_json = self._read_json(skipped_plan)
        self.assertTrue(skipped_json["is_bc"])
        self.assertTrue(skipped_json["skip_backend"])
        self.assertTrue(os.path.exists(skipped_output))

        empty_json = self._read_json(empty_plan)
        self.assertTrue(empty_json["is_bc"])
        self.assertFalse(empty_json["skip_backend"])
        self.assertEqual([], empty_json["imports"])

        native_json = self._read_json(native_plan)
        self.assertFalse(native_json["is_bc"])
        self.assertFalse(native_json["skip_backend"])

        self.assertEqual(
            {"index": {"0": 1, "2": 1}, "non_lto_objects": {"3": 1}},
            self._read_json(self.link_plan_path),
        )
        self.assertEqual(
            [
                selected_output.replace(".thinlto.bc", ".opt.o"),
                empty_output.replace(".thinlto.bc", ".opt.o"),
                native_path,
            ],
            self._read_lines(self.final_link_index_path),
        )

    def test_mixed_archive_marks_skipped_member_for_omission(self):
        archive, paths, plan, indexes = self._archive_linkable(
            ["selected", "skipped", "native"]
        )
        selected_path, skipped_path, native_path = paths
        self._emit_bitcode_outputs(selected_path)
        self._emit_bitcode_outputs(skipped_path)

        self._run_planner(
            [archive],
            [selected_path],
            [self._indexed_path(selected_path), native_path],
        )

        object_plans = {
            entry["path"]: entry for entry in self._read_json(plan)["objects"]
        }
        self.assertFalse(object_plans[selected_path]["skip_backend"])
        self.assertTrue(object_plans[selected_path]["is_bc"])
        self.assertTrue(object_plans[skipped_path]["skip_backend"])
        self.assertTrue(object_plans[skipped_path]["is_bc"])
        self.assertNotIn("bitcode_file", object_plans[skipped_path])
        self.assertNotIn("imports", object_plans[skipped_path])
        self.assertFalse(object_plans[native_path]["skip_backend"])
        self.assertFalse(object_plans[native_path]["is_bc"])
        self.assertFalse(
            os.path.exists(
                os.path.join(indexes, os.path.basename(skipped_path) + ".thinlto.bc")
            )
        )
        self.assertEqual(
            [
                selected_path.replace("/objects/", "/opt_objects/objects/"),
                native_path.replace("/objects/", "/opt_objects/objects/"),
            ],
            self._read_lines(self.final_link_index_path),
        )

    def test_all_skipped_archive_has_no_linked_objects(self):
        archive, paths, plan, _ = self._archive_linkable(["first", "second"])
        for path in paths:
            self._emit_bitcode_outputs(path)

        self._run_planner([archive], [], [])

        object_plans = self._read_json(plan)["objects"]
        self.assertTrue(all(entry["is_bc"] for entry in object_plans))
        self.assertTrue(all(entry["skip_backend"] for entry in object_plans))
        self.assertEqual(
            {"index": {}, "non_lto_objects": {}},
            self._read_json(self.link_plan_path),
        )
        self.assertEqual([], self._read_lines(self.final_link_index_path))

    def test_normalizes_linked_index_paths(self):
        selected, selected_path, selected_output, selected_plan = self._direct_linkable(
            "selected", 0
        )
        self._emit_bitcode_outputs(selected_path)
        noncanonical_path = os.path.join("objects", "..", selected_path)

        self._run_planner(
            [selected],
            [noncanonical_path],
            [self._indexed_path(noncanonical_path)],
        )

        self.assertFalse(self._read_json(selected_plan)["skip_backend"])
        self.assertEqual(
            [selected_output.replace(".thinlto.bc", ".opt.o")],
            self._read_lines(self.final_link_index_path),
        )

    def test_rejects_colliding_normalized_input_paths(self):
        first, first_path, _, _ = self._direct_linkable("first", 0)
        second, _, _, _ = self._direct_linkable("second", 1)
        second["path"] = os.path.join("objects", "nested", "..", "first.o")

        with self.assertRaisesRegex(
            RuntimeError, "ThinLTO inputs normalize to the same path"
        ):
            self._run_planner([first, second], [first_path], [])

    def test_selected_direct_module_requires_imports_file(self):
        selected, selected_path, _, _ = self._direct_linkable("selected", 0)
        self._write(self._indexed_path(selected_path) + ".thinlto.bc", "thinlto index")

        with self.assertRaisesRegex(
            AssertionError, "selected ThinLTO module without imports file"
        ):
            self._run_planner(
                [selected],
                [selected_path],
                [self._indexed_path(selected_path)],
            )

    def test_selected_archive_module_requires_bitcode_index(self):
        archive, paths, _, _ = self._archive_linkable(["selected"])
        selected_path = paths[0]
        self._write(self._indexed_path(selected_path) + ".imports", "")

        with self.assertRaisesRegex(
            AssertionError, "selected ThinLTO module without bitcode index"
        ):
            self._run_planner(
                [archive],
                [selected_path],
                [self._indexed_path(selected_path)],
            )

    def test_skipped_whole_archive_member_fails(self):
        archive, paths, _, _ = self._archive_linkable(["skipped"], link_whole=True)
        self._emit_bitcode_outputs(paths[0])

        with self.assertRaisesRegex(
            AssertionError, "LLD skipped whole-archive ThinLTO member"
        ):
            self._run_planner([archive], [], [])

    def test_linked_direct_bitcode_without_summary_fails(self):
        # LLD leaves a zero-byte index and imports file for bitcode it linked
        # without a ThinLTO summary, and lists it only in index.full.
        linked, linked_path, _, _ = self._direct_linkable("full_lto", 0)
        self._write(self._indexed_path(linked_path) + ".imports", "")
        self._write(self._indexed_path(linked_path) + ".thinlto.bc", "")

        with self.assertRaisesRegex(
            AssertionError,
            "linked bitcode without a ThinLTO summary: objects/full_lto.o",
        ):
            self._run_planner([linked], [], [self._indexed_path(linked_path)])

    def test_linked_archive_bitcode_without_summary_fails(self):
        archive, paths, _, _ = self._archive_linkable(["full_lto"])
        self._write(self._indexed_path(paths[0]) + ".imports", "")
        self._write(self._indexed_path(paths[0]) + ".thinlto.bc", "")

        with self.assertRaisesRegex(
            AssertionError, "linked bitcode without a ThinLTO summary"
        ):
            self._run_planner([archive], [], [self._indexed_path(paths[0])])

    def test_skipped_member_in_full_index_fails(self):
        skipped, skipped_path, _, _ = self._direct_linkable("skipped", 0)
        self._emit_bitcode_outputs(skipped_path)

        with self.assertRaisesRegex(
            AssertionError, "LLD listed skipped ThinLTO member in index.full"
        ):
            self._run_planner(
                [skipped],
                [],
                [self._indexed_path(skipped_path)],
            )
