# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Tests the class lists that consolidate_class_names.py writes: from class names files, or from the
`.class` entries of each target's jars, read as Java's ClasspathTraversal reads them (--input-jars); the class
names files of --class-names-mapping-file, added per target; its duplicate check, with and without
--skip-duplicate-check; and that mapping values must match the mode."""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
import warnings
import zipfile
from unittest import mock

from android.tools import consolidate_class_names


class ConsolidateClassNamesTest(unittest.TestCase):
    def _write(self, directory: str, name: str, content: str) -> str:
        path = os.path.join(directory, name)
        with open(path, "w") as f:
            f.write(content)
        return path

    def _jar(self, directory: str, name: str, *entries: str) -> str:
        path = os.path.join(directory, name)
        with zipfile.ZipFile(path, "w") as jar, warnings.catch_warnings():
            # zipfile warns on a duplicate name.
            warnings.simplefilter("ignore")
            for entry in entries:
                jar.writestr(entry, b"")
        return path

    def _run(
        self, directory: str, mapping: dict[str, str | list[str]], *extra: str
    ) -> dict[str, list[str]]:
        mapping_file = self._write(directory, "mapping.json", json.dumps(mapping))
        output = os.path.join(directory, "out.json")
        argv = [
            "consolidate_class_names",
            "--input-mapping-file",
            mapping_file,
            "--output-file",
            output,
            *extra,
        ]
        with mock.patch.object(sys, "argv", argv):
            consolidate_class_names.main()
        with open(output) as f:
            return json.load(f)

    def test_plain_paths(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            a = self._write(d, "a.txt", "com/a/A\ncom/a/A$1\n")
            self.assertEqual(
                self._run(d, {"//a:a": a}), {"//a:a": ["com/a/A", "com/a/A$1"]}
            )

    def test_duplicates_fail_unless_skipped(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            a = self._write(d, "a.txt", "com/x/X\n")
            b = self._write(d, "b.txt", "com/x/X\n")
            with self.assertRaises(SystemExit):
                self._run(d, {"//a:a": a, "//b:b": b})
            self.assertEqual(
                self._run(d, {"//a:a": a, "//b:b": b}, "--skip-duplicate-check"),
                {"//a:a": ["com/x/X"], "//b:b": ["com/x/X"]},
            )

    def test_input_jars_lists_class_entries_in_zip_order(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            jar = self._jar(
                d,
                "a.jar",
                "com/a/",
                "com/a/B.class",
                "com/a/res.txt",
                "module-info.class",
                "META-INF/versions/9/x/Y.class",
                "com/a/A.class",
                "com/a/B.class",
            )
            self.assertEqual(
                self._run(
                    d, {"//a:a": [jar]}, "--input-jars", "--skip-duplicate-check"
                ),
                {
                    "//a:a": [
                        "com/a/B",
                        "module-info",
                        "META-INF/versions/9/x/Y",
                        "com/a/A",
                        "com/a/B",
                    ]
                },
            )

    def test_input_jars_lists_every_jar_of_a_target_in_order(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            first = self._jar(d, "first.jar", "com/a/B.class")
            second = self._jar(d, "second.jar", "com/a/res.txt", "com/a/A.class")
            self.assertEqual(
                self._run(d, {"//a:a": [first, second]}, "--input-jars"),
                {"//a:a": ["com/a/B", "com/a/A"]},
            )

    def test_input_jars_reads_paths_as_classpath_traversal_does(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            classes = os.path.join(d, "classes")
            os.makedirs(os.path.join(classes, "com", "x"))
            self._write(os.path.join(classes, "com", "x"), "X.class", "")
            self._write(classes, "res.txt", "")
            mapping = {
                "//missing:m": [os.path.join(d, "missing.jar")],
                "//directory:d": [classes],
                "//jmod:j": [self._jar(d, "j.jmod", "classes/com/j/J.class")],
                "//upper:u": [self._jar(d, "u.JAR", "com/u/U.class")],
            }
            self.assertEqual(
                self._run(d, mapping, "--input-jars"),
                {
                    "//missing:m": [],
                    "//directory:d": ["com/x/X"],
                    "//jmod:j": [],
                    "//upper:u": ["com/u/U"],
                },
            )

    def test_input_jars_decodes_names_as_utf8_without_the_utf8_flag(self) -> None:
        # zipfile sets the UTF-8 flag for any non-ASCII name, so write an ASCII name of the same length
        # and put the UTF-8 bytes in its place afterwards.
        name = "com/ü/Ü.class".encode("utf-8")
        placeholder = b"com/uu/UU.class"
        self.assertEqual(len(name), len(placeholder))
        with tempfile.TemporaryDirectory() as d:
            jar = self._jar(d, "a.jar", placeholder.decode(), "com/ü/V.class")
            with open(jar, "rb") as f:
                data = f.read()
            with open(jar, "wb") as f:
                f.write(data.replace(placeholder, name))
            with zipfile.ZipFile(jar) as z:
                self.assertEqual(
                    [info.flag_bits & 0x800 for info in z.infolist()], [0, 0x800]
                )
            self.assertEqual(
                self._run(d, {"//a:a": [jar]}, "--input-jars"),
                {"//a:a": ["com/ü/Ü", "com/ü/V"]},
            )

    def test_input_jars_duplicates_fail_unless_skipped(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            a = self._jar(d, "a.jar", "com/x/X.class")
            b = self._jar(d, "b.jar", "com/x/X.class")
            with self.assertRaises(SystemExit):
                self._run(d, {"//a:a": [a], "//b:b": [b]}, "--input-jars")
            self.assertEqual(
                self._run(
                    d,
                    {"//a:a": [a], "//b:b": [b]},
                    "--input-jars",
                    "--skip-duplicate-check",
                ),
                {"//a:a": ["com/x/X"], "//b:b": ["com/x/X"]},
            )

    def test_class_names_mapping_adds_class_names_files_per_target(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            jar = self._jar(d, "a.jar", "com/a/A.class")
            a_names = self._write(d, "a_names.txt", "com/a/B\n")
            b_names = self._write(d, "b_names.txt", "com/b/C\ncom/b/C$1\n")
            class_names_mapping = self._write(
                d,
                "class_names.json",
                json.dumps({"//a:a": [a_names], "//b:b": [b_names]}),
            )
            self.assertEqual(
                self._run(
                    d,
                    {"//a:a": [jar]},
                    "--input-jars",
                    "--class-names-mapping-file",
                    class_names_mapping,
                ),
                {"//a:a": ["com/a/A", "com/a/B"], "//b:b": ["com/b/C", "com/b/C$1"]},
            )

    def test_class_names_mapping_duplicates_fail_unless_skipped(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            jar = self._jar(d, "a.jar", "com/x/X.class")
            b_names = self._write(d, "b_names.txt", "com/x/X\n")
            class_names_mapping = self._write(
                d, "class_names.json", json.dumps({"//b:b": [b_names]})
            )
            args = ["--input-jars", "--class-names-mapping-file", class_names_mapping]
            with self.assertRaises(SystemExit):
                self._run(d, {"//a:a": [jar]}, *args)
            self.assertEqual(
                self._run(d, {"//a:a": [jar]}, *args, "--skip-duplicate-check"),
                {"//a:a": ["com/x/X"], "//b:b": ["com/x/X"]},
            )

    def test_mapping_values_must_match_the_mode(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            jar = self._jar(d, "a.jar", "com/a/A.class")
            with self.assertRaisesRegex(SystemExit, "Unexpected mapping value"):
                self._run(d, {"//a:a": jar}, "--input-jars")
            with self.assertRaisesRegex(SystemExit, "Unexpected mapping value"):
                self._run(d, {"//a:a": [jar]})


if __name__ == "__main__":
    unittest.main()
