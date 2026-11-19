# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import os
import tempfile
import unittest
from pathlib import Path
from typing import Generator

from .assemble_bundle import assemble_bundle
from .assemble_bundle_types import BundleSpecItem, IncrementalContext
from .incremental_state import IncrementalState

try:
    from contextlib import chdir  # pyre-ignore[21], Python 3.11+
except ImportError:
    from contextlib import contextmanager

    @contextmanager
    def chdir(path: os.PathLike) -> Generator[None, None, None]:
        cwd = os.getcwd()
        try:
            os.chdir(path)
            yield
        finally:
            os.chdir(cwd)


def _write_versioned_framework(framework: Path) -> None:
    (framework / "Versions" / "A" / "Resources").mkdir(parents=True)
    (framework / "Versions" / "A" / "Foo").write_text("binary")
    (framework / "Versions" / "A" / "Resources" / "Info.plist").write_text("plist")
    os.symlink("A", framework / "Versions" / "Current")
    os.symlink("Versions/Current/Foo", framework / "Foo")
    os.symlink("Versions/Current/Resources", framework / "Resources")


def _context(state) -> IncrementalContext:
    return IncrementalContext(
        metadata={},
        state=state,
        codesigned=False,
        codesign_configuration=None,
        codesign_identity=None,
        codesign_arguments=[],
        versioned_if_macos=False,
    )


def _layout(root: Path) -> set:
    entries = set()
    for current_dir, dir_names, file_names in os.walk(root):
        for name in dir_names + file_names:
            path = Path(current_dir) / name
            relative = str(path.relative_to(root))
            if path.is_symlink():
                entries.add((relative, "symlink", os.readlink(path)))
            elif path.is_dir():
                entries.add((relative, "directory", ""))
            else:
                entries.add((relative, "file", path.read_text()))
    return entries


class TestIncrementalSymlinks(unittest.TestCase):
    def _bundle_main_then_add_framework(self, root: Path) -> None:
        """Bundles `main` alone, then `main` plus a versioned framework on top of it
        incrementally, and the same spec non-incrementally into a second bundle."""
        with chdir(root):
            Path("src").mkdir()
            Path("src/main").write_text("main")
            _write_versioned_framework(Path("src/Foo.framework"))
            main_only = [BundleSpecItem(src="src/main", dst="Contents/MacOS/main")]
            fresh = _context(None)
            items = assemble_bundle(main_only, Path("a.app"), fresh, True, False)
            state = IncrementalState(
                items=items,
                codesigned=False,
                codesign_configuration=None,
                codesigned_on_copy=[],
                codesign_identity=None,
                codesign_arguments=[],
                swift_stdlib_paths=[],
                versioned_if_macos=False,
            )
            framework = BundleSpecItem(
                src="src/Foo.framework", dst="Contents/Frameworks/Foo.framework"
            )
            with_framework = sorted(main_only + [framework])
            assemble_bundle(with_framework, Path("a.app"), _context(state), True, False)
            assemble_bundle(with_framework, Path("b.app"), None, True, False)

    def test_adding_a_versioned_framework_matches_a_fresh_bundle(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            self._bundle_main_then_add_framework(root)
            incremental = _layout(root / "a.app")
            self.assertEqual(incremental, _layout(root / "b.app"))
            framework = "Contents/Frameworks/Foo.framework"
            current = (f"{framework}/Versions/Current", "symlink", "A")
            self.assertIn(current, incremental)
            self.assertIn(
                (f"{framework}/Resources", "symlink", "Versions/Current/Resources"),
                incremental,
            )
            self.assertEqual((root / "a.app" / framework / "Foo").read_text(), "binary")
