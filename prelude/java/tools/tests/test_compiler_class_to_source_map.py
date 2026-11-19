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
import zipfile

from compiler_class_to_source_map import generate_class_to_source_map


class CompilerClassToSourceMapTest(unittest.TestCase):
    def generate(self, classes, mapping, sources, prefixes=(), sources_jar=None):
        with tempfile.TemporaryDirectory() as directory:
            jar = pathlib.Path(directory) / "final.jar"
            with zipfile.ZipFile(jar, "w") as archive:
                for name in classes:
                    archive.writestr(name + ".class", b"")
            report = pathlib.Path(directory) / "compiler.json"
            report.write_text(
                json.dumps(
                    {name + ".class": origins for name, origins in mapping.items()}
                )
            )
            output = io.StringIO()
            generate_class_to_source_map(
                str(jar),
                str(report),
                sources,
                output,
                include_classes_prefixes=prefixes,
                owner_target="cell//package:library",
                sources_jar=sources_jar,
            )
            result = json.loads(output.getvalue())
            self.assertEqual(result["jarPath"], str(jar))
            self.assertEqual(result["ownerTarget"], "cell//package:library")
            return result["classes"]

    def test_bodyless_data_classes_in_differently_named_file(self):
        self.assertEqual(
            self.generate(
                ["p/Settings", "p/Profile"],
                {"p/Settings": ["src/Models.kt"], "p/Profile": ["src/Models.kt"]},
                ["src/Models.kt"],
            ),
            [
                {"className": "p.Settings", "srcPath": "src/Models.kt"},
                {"className": "p.Profile", "srcPath": "src/Models.kt"},
                {"className": "p.Models", "srcPath": "src/Models.kt"},
            ],
        )

    def test_filters_final_jar_and_preserves_unknown_source_policy(self):
        self.assertEqual(
            self.generate(
                ["p/Kept", "generated/New", "generated/New$Inner", "p/Unmapped"],
                {"p/Kept": ["src/Models.java"], "p/Removed": ["src/Removed.java"]},
                ["src/Models.java", "src/Removed.java", "p/Unmapped.java"],
                prefixes=("generated.",),
            ),
            [
                {"className": "p.Kept", "srcPath": "src/Models.java"},
                {"className": "generated.New"},
                {"className": "p.Models", "srcPath": "src/Models.java"},
            ],
        )

    def test_preserves_nested_classes_and_kotlin_facade_compatibility(self):
        self.assertEqual(
            self.generate(
                ["p/Outer$1", "p/FunctionsKt", "p/RealKt"],
                {
                    "p/Outer$1": ["Outer.java"],
                    "p/FunctionsKt": ["Functions.kt"],
                    "p/RealKt": ["RealKt.kt"],
                },
                ["Outer.java", "Functions.kt", "RealKt.kt"],
            ),
            [
                {"className": "p.Outer", "srcPath": "Outer.java"},
                {"className": "p.Functions", "srcPath": "Functions.kt"},
                {"className": "p.RealKt", "srcPath": "RealKt.kt"},
            ],
        )

    def test_does_not_guess_ambiguous_or_scratch_origins(self):
        self.assertEqual(
            self.generate(
                ["p/Facade", "p/Generated"],
                {
                    "p/Facade": ["A.kt", "B.kt"],
                    "p/Generated": ["scratch/Generated.java"],
                },
                ["A.kt", "B.kt", "p/Generated.java"],
            ),
            [],
        )

    def test_packages_each_declared_source_once(self):
        with tempfile.TemporaryDirectory() as directory:
            source = pathlib.Path(directory) / "Models.kt"
            source.write_text("data class A(val n: Int); data class B(val n: Int)")
            output = pathlib.Path(directory) / "sources.jar"
            self.generate(
                ["p/A", "p/B"],
                {"p/A": [str(source)], "p/B": [str(source)]},
                [str(source)],
                sources_jar=str(output),
            )
            with zipfile.ZipFile(output) as archive:
                self.assertEqual(archive.namelist(), ["p/Models.kt"])
                self.assertEqual(
                    archive.read("p/Models.kt").decode(), source.read_text()
                )
