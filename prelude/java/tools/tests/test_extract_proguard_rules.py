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

from extract_proguard_rules import extract


def _extract(entries: dict[str, bytes]) -> str:
    with tempfile.TemporaryDirectory() as d:
        jar = pathlib.Path(d) / "lib.jar"
        out = pathlib.Path(d) / "rules.pro"
        with zipfile.ZipFile(jar, "w") as zf:
            for name, content in entries.items():
                zf.writestr(name, content)
        extract(str(jar), str(out))
        return out.read_text()


class ExtractProguardRulesTest(unittest.TestCase):
    def test_legacy_layout(self):
        rules = _extract(
            {
                "META-INF/proguard/a.pro": b"-dontwarn a.**",
                "com/example/Foo.class": b"",
            }
        )
        self.assertIn("-dontwarn a.**", rules)

    def test_android_tools_layout_takes_only_proguard_files(self):
        rules = _extract(
            {
                "META-INF/proguard/legacy.pro": b"-dontwarn legacy.**",
                "META-INF/com.android.tools/r8/r8.pro": b"-dontwarn r8.**",
                "META-INF/com.android.tools/proguard/pg.pro": b"-dontwarn pg.**",
            }
        )
        self.assertIn("-dontwarn pg.**", rules)
        self.assertNotIn("legacy", rules)
        self.assertNotIn("r8", rules)

    def test_android_tools_layout_without_proguard_files(self):
        rules = _extract(
            {
                "META-INF/proguard/legacy.pro": b"-dontwarn legacy.**",
                "META-INF/com.android.tools/r8/r8.pro": b"-dontwarn r8.**",
            }
        )
        self.assertEqual(rules, "")

    def test_no_rules(self):
        self.assertEqual(_extract({"com/example/Foo.class": b""}), "")
