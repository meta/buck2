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

from cxx.dist_lto.tools import dist_lto_planner_darwin as planner


class DistLtoPlannerDarwinTest(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root = self.temp_dir.name
        self.index_dir = os.path.join(self.root, "index")
        self.meta_path = os.path.join(self.root, "meta.json")
        self.link_plan_path = os.path.join(self.root, "link-plan.json")
        os.makedirs(self.index_dir)

    def tearDown(self):
        self.temp_dir.cleanup()

    def _write(self, path, contents):
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as outfile:
            outfile.write(contents)

    def _read_json(self, path):
        with open(path) as infile:
            return json.load(infile)

    def _indexed_path(self, path):
        return os.path.join(self.index_dir, path)

    def _record(self, name, idx):
        return {
            "input_object_file_path": "objects/{}.o".format(name),
            "output_index_shard_file_path": os.path.join(
                self.root, "out", name + ".thinlto.bc"
            ),
            "output_plan_file_path": os.path.join(self.root, "out", name + ".opt.plan"),
            "starlark_array_index": idx,
        }

    def _emit_bitcode_outputs(self, record, imports=()):
        path = record["input_object_file_path"]
        self._write(self._indexed_path(path) + ".imports", "\n".join(imports))
        self._write(self._indexed_path(path) + ".thinlto.bc", "thinlto index")

    def _run_planner(self, records, full_index_lines):
        self._write(self.meta_path, json.dumps(records))
        self._write(
            os.path.join(self.index_dir, "index.full"),
            "\n".join(full_index_lines),
        )
        argv = [
            "dist_lto_planner_darwin.py",
            "--meta",
            self.meta_path,
            "--index",
            self.index_dir,
            "--link-plan",
            self.link_plan_path,
            "--",
            "fake-linker",
            "@argsfile",
        ]
        with (
            contextlib.chdir(self.root),
            mock.patch.object(planner.subprocess, "check_call") as check_call,
        ):
            self.assertIsNone(planner.main(argv))
        check_call.assert_called_once_with(["fake-linker", "@argsfile"])

    def test_bitcode_unloaded_bitcode_and_native_objects(self):
        loaded = self._record("loaded", 0)
        unloaded = self._record("unloaded", 1)
        native = self._record("native", 2)
        self._emit_bitcode_outputs(loaded, [unloaded["input_object_file_path"]])
        self._emit_bitcode_outputs(unloaded)

        self._run_planner(
            [loaded, unloaded, native],
            [
                native["input_object_file_path"],
                self._indexed_path(loaded["input_object_file_path"]),
            ],
        )

        self.assertEqual(
            {
                "final_link_line_position": 1,
                "imports": [1],
                "is_bitcode": True,
                "loaded_by_linker": True,
            },
            self._read_json(loaded["output_plan_file_path"]),
        )
        self.assertEqual(
            {
                "final_link_line_position": -1,
                "imports": [],
                "is_bitcode": True,
                "loaded_by_linker": False,
            },
            self._read_json(unloaded["output_plan_file_path"]),
        )
        self.assertEqual(
            {
                "final_link_line_position": 0,
                "imports": [],
                "is_bitcode": False,
                "loaded_by_linker": True,
            },
            self._read_json(native["output_plan_file_path"]),
        )
        self.assertTrue(os.path.exists(loaded["output_index_shard_file_path"]))
        self.assertTrue(os.path.exists(native["output_index_shard_file_path"]))
        self.assertEqual(
            {"non_lto_objects": {"2": 1}},
            self._read_json(self.link_plan_path),
        )
