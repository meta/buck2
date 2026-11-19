# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

CREATE_LINK_TREE: Path = Path(__file__).resolve().parent.parent / "create_link_tree.py"


def _init_files_of_link_tree(tmp: Path, dests: list[str]) -> list[str]:
    src = tmp / "src"
    manifest = []
    for dest in dests:
        path = src / dest
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("", encoding="utf8")
        manifest.append([dest, str(path), "//x:x"])
    manifest_path = tmp / "manifest.json"
    manifest_path.write_text(json.dumps(manifest), encoding="utf8")
    out = tmp / "tree.d"
    proc = subprocess.run(
        [
            sys.executable,
            "-B",
            str(CREATE_LINK_TREE),
            "--output",
            str(out),
            "--link-manifest",
            str(manifest_path),
        ],
        capture_output=True,
        encoding="utf8",
    )
    if proc.returncode != 0:
        raise AssertionError(proc.stderr)
    return sorted(str(p.relative_to(out)) for p in out.rglob("__init__.py"))


class LinkTreeInitFilesTest(unittest.TestCase):
    def test_a_top_level_module_adds_an_init_file_at_the_tree_root(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp = Path(tmpdir)
            inits = _init_files_of_link_tree(tmp, ["foo.py", "pkg/mod.py"])
        self.assertEqual(inits, ["__init__.py", "pkg/__init__.py"])

    def test_a_top_level_extension_adds_an_init_file_at_the_tree_root(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp = Path(tmpdir)
            inits = _init_files_of_link_tree(tmp, ["ext.so"])
        self.assertEqual(inits, ["__init__.py"])
