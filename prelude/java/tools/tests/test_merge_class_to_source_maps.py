#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import io
import json
import pathlib
import tempfile
import unittest

from merge_class_to_source_maps import merge_class_to_source_maps


class MergeClassToSourceMapsTest(unittest.TestCase):
    def test_relativizes_paths_and_preserves_owner_and_sourceless_classes(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            directory = pathlib.Path(temp_dir)
            mapping_path = directory / "mapping.json"
            mapping_path.write_text(
                json.dumps(
                    {
                        "jarPath": str(directory / "library.jar"),
                        "ownerTarget": "cell//package:library",
                        "classes": [
                            {
                                "className": "com.example.Sourceful",
                                "srcPath": str(directory / "Sourceful.java"),
                            },
                            {"className": "com.example.Generated"},
                        ],
                    }
                )
            )
            output = io.StringIO()

            merge_class_to_source_maps([str(mapping_path)], output, str(directory))

        self.assertEqual(
            json.loads(output.getvalue()),
            {
                "jarPath": "library.jar",
                "ownerTarget": "cell//package:library",
                "classes": [
                    {
                        "className": "com.example.Sourceful",
                        "srcPath": "Sourceful.java",
                    },
                    {"className": "com.example.Generated"},
                ],
            },
        )
