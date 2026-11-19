# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import json
import sys
import tempfile
import unittest
from pathlib import Path

import make_py_package_manifest_module


class MakePyPackageManifestModuleTest(unittest.TestCase):
    def test_merge_entries(self) -> None:
        entries: dict[str, object] = {
            "fbmake": {"preserved": "base", "replaced": "old"},
            "replaced": "old",
        }

        make_py_package_manifest_module.merge_entries(
            entries,
            {
                "fbmake": {"replaced": "new"},
                "new": "value",
                "replaced": {"nested": True},
            },
        )

        self.assertEqual(
            {
                "fbmake": {"preserved": "base", "replaced": "new"},
                "new": "value",
                "replaced": {"nested": True},
            },
            entries,
        )

    def test_main_writes_merged_json_before_module_entries(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            entries = root / "entries.json"
            overlay = root / "overlay.json"
            module_manifest = root / "module-manifest.json"
            module_manifests = root / "module-manifests.txt"
            output = root / "__manifest__.py"
            output_json = root / "__manifest__.json"
            entries.write_text(json.dumps({"fbmake": {"preserved": "base"}}))
            overlay.write_text(json.dumps({"fbmake": {"revision": "fresh"}}))
            module_manifest.write_text(
                json.dumps([["pkg/module.py", "unused", "origin"]])
            )
            module_manifests.write_text(str(module_manifest) + "\n")

            old_argv = sys.argv
            sys.argv = [
                "make_py_package_manifest_module.py",
                "--manifest-entries",
                str(entries),
                "--manifest-entries-overlay",
                str(overlay),
                "--module-manifests",
                str(module_manifests),
                "--output",
                str(output),
                "--output-json",
                str(output_json),
            ]
            try:
                make_py_package_manifest_module.main()
            finally:
                sys.argv = old_argv

            self.assertEqual(
                {"fbmake": {"preserved": "base", "revision": "fresh"}},
                json.loads(output_json.read_text()),
            )
            module_text = output.read_text()
            self.assertIn("'pkg.module'", module_text)
            self.assertIn("'pkg.__init__'", module_text)


class ModuleNamesTest(unittest.TestCase):
    def _modules_for(self, manifest: list[list[str]]) -> list[str]:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            module_manifest = root / "module-manifest.json"
            module_manifests = root / "module-manifests.txt"
            output = root / "__manifest__.py"
            module_manifest.write_text(json.dumps(manifest))
            module_manifests.write_text(str(module_manifest) + "\n")
            old_argv = sys.argv
            sys.argv = [
                "make_py_package_manifest_module.py",
                "--module-manifests",
                str(module_manifests),
                "--output",
                str(output),
            ]
            try:
                make_py_package_manifest_module.main()
            finally:
                sys.argv = old_argv
            namespace: dict[str, object] = {}
            exec(output.read_text(), namespace)
            return list(namespace["modules"])  # pyre-ignore[6]

    def test_extensions_lose_their_abi_tag_and_resource_dirs_get_no_init(self) -> None:
        modules = self._modules_for(
            [
                ["pkg/__init__.py", "src", "origin"],
                ["pkg/_speedups.cpython-312-x86_64-linux-gnu.so", "src", "origin"],
                ["pkg/data/table.json", "src", "origin"],
            ]
        )
        self.assertEqual(modules, ["pkg.__init__", "pkg._speedups"])

    def test_static_extension_stubs_get_a_package_init(self) -> None:
        modules = self._modules_for(
            [
                ["pkg/native/ext.empty_stub", "src", "origin"],
                ["pkg/native/ext.pyi", "src", "origin"],
            ]
        )
        self.assertEqual(modules, ["pkg.__init__", "pkg.native.__init__"])
