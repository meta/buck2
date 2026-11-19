#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import pathlib
import tempfile
import unittest
import zipfile

from list_class_names import _get_class_names


def _create_jar(path, class_files):
    with zipfile.ZipFile(path, "w") as zf:
        for cf in class_files:
            zf.writestr(cf, b"")


def _create_sources(path, sources):
    path.write_text("\n".join(sources) + "\n")


class ListClassNamesTest(unittest.TestCase):
    def test_java_stem_matching(self):
        with tempfile.TemporaryDirectory() as d:
            d = pathlib.Path(d)
            jar = d / "test.jar"
            sources = d / "sources.txt"
            _create_jar(jar, ["com/example/Foo.class", "com/example/Bar.class"])
            _create_sources(sources, ["src/com/example/Foo.java"])
            result = _get_class_names(sources, jar)
            self.assertEqual(result, ["com.example.Foo"])

    def test_java_no_match(self):
        with tempfile.TemporaryDirectory() as d:
            d = pathlib.Path(d)
            jar = d / "test.jar"
            sources = d / "sources.txt"
            _create_jar(jar, ["com/example/Foo.class"])
            _create_sources(sources, ["src/com/example/Bar.java"])
            result = _get_class_names(sources, jar)
            self.assertEqual(result, [])

    def test_inner_classes_excluded(self):
        with tempfile.TemporaryDirectory() as d:
            d = pathlib.Path(d)
            jar = d / "test.jar"
            sources = d / "sources.txt"
            _create_jar(jar, ["com/example/Foo.class", "com/example/Foo$Inner.class"])
            _create_sources(sources, ["src/com/example/Foo.java"])
            result = _get_class_names(sources, jar)
            self.assertEqual(result, ["com.example.Foo"])

    def test_discover_all_includes_all_top_level_classes(self):
        with tempfile.TemporaryDirectory() as d:
            d = pathlib.Path(d)
            jar = d / "test.jar"
            sources = d / "sources.txt"
            _create_jar(
                jar,
                [
                    "com/example/Foo.class",
                    "com/example/Bar.class",
                    "com/example/Baz.class",
                    "com/example/Foo$Inner.class",
                ],
            )
            _create_sources(sources, ["src/com/example/Main.kt"])
            result = _get_class_names(sources, jar, discover_all=True)
            self.assertEqual(
                sorted(result),
                ["com.example.Bar", "com.example.Baz", "com.example.Foo"],
            )

    def test_discover_all_still_excludes_inner_classes(self):
        with tempfile.TemporaryDirectory() as d:
            d = pathlib.Path(d)
            jar = d / "test.jar"
            sources = d / "sources.txt"
            _create_jar(
                jar,
                ["com/example/Foo.class", "com/example/Foo$Inner.class"],
            )
            _create_sources(sources, ["src/com/example/Foo.kt"])
            result = _get_class_names(sources, jar, discover_all=True)
            self.assertEqual(result, ["com.example.Foo"])

    def test_discover_all_ignores_non_class_files(self):
        with tempfile.TemporaryDirectory() as d:
            d = pathlib.Path(d)
            jar = d / "test.jar"
            sources = d / "sources.txt"
            _create_jar(
                jar,
                [
                    "com/example/Foo.class",
                    "META-INF/MANIFEST.MF",
                    "com/example/resource.txt",
                ],
            )
            _create_sources(sources, ["src/com/example/Foo.kt"])
            result = _get_class_names(sources, jar, discover_all=True)
            self.assertEqual(result, ["com.example.Foo"])

    def test_empty_sources(self):
        with tempfile.TemporaryDirectory() as d:
            d = pathlib.Path(d)
            jar = d / "test.jar"
            sources = d / "sources.txt"
            _create_jar(jar, ["com/example/Foo.class"])
            _create_sources(sources, [])
            result = _get_class_names(sources, jar)
            self.assertEqual(result, [])

    def test_discover_all_with_empty_jar(self):
        with tempfile.TemporaryDirectory() as d:
            d = pathlib.Path(d)
            jar = d / "test.jar"
            sources = d / "sources.txt"
            _create_jar(jar, [])
            _create_sources(sources, ["src/Foo.kt"])
            result = _get_class_names(sources, jar, discover_all=True)
            self.assertEqual(result, [])


if __name__ == "__main__":
    unittest.main()
