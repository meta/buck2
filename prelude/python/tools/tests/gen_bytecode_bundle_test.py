# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


from __future__ import annotations

import json
import marshal
import os
import py_compile
import tempfile
import tracemalloc
import unittest
from dataclasses import dataclass
from importlib.util import cache_from_source, MAGIC_NUMBER
from pathlib import Path
from types import CodeType

import gen_bytecode_bundle


@dataclass
class Library:
    """What one `python_library`'s `compile.py` action leaves behind."""

    name: str
    manifest: Path
    bytecode_dir: Path
    # Keyed by source path in the tree.
    pycs: dict[str, Path]


@dataclass
class Bundle:
    """A built bundle, split back into its four sections."""

    magic: bytes
    pyc_magic: bytes
    index: dict[str, tuple[int, int]]
    payload: bytes

    def code(self, source_relpath: str) -> CodeType:
        offset, size = self.index[source_relpath]
        code = marshal.loads(self.payload[offset : offset + size])
        assert isinstance(code, CodeType)
        return code

    def run(self, source_relpath: str) -> dict[str, object]:
        """Execute a bundled module and return its globals."""
        namespace: dict[str, object] = {}
        exec(self.code(source_relpath), namespace)
        return namespace


class BundleTestBase(unittest.TestCase):
    def setUp(self) -> None:
        tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(tmpdir.cleanup)
        self.root = Path(tmpdir.name)

    def _library(
        self, name: str, sources: dict[str, str], pyc_field: str | None = None
    ) -> Library:
        """Compile one `python_library` the way `compile.py` does.

        Leaves a `__pycache__` tree under the action's output directory, plus a
        manifest of `(destination, .pyc path, source)` for what it wrote.
        `pyc_field` overrides that middle field, which is where a real build
        puts a content-hash placeholder.
        """
        bytecode_dir = self.root / name / "bytecode"
        entries = []
        pycs = {}
        for source_relpath, source in sources.items():
            src = self.root / name / "src" / source_relpath
            src.parent.mkdir(parents=True, exist_ok=True)
            src.write_text(source, encoding="utf8")

            dest = cache_from_source(source_relpath)
            pyc = bytecode_dir / dest
            pyc.parent.mkdir(parents=True, exist_ok=True)
            py_compile.compile(
                str(src), cfile=str(pyc), dfile=source_relpath, doraise=True
            )
            entries.append([dest, pyc_field or str(pyc), str(src)])
            pycs[source_relpath] = pyc

        manifest = self.root / name / "bytecode.manifest"
        manifest.write_text(json.dumps(entries), encoding="utf8")
        return Library(name, manifest, bytecode_dir, pycs)

    def _build(self, *libraries: Library) -> Path:
        """Bundle `libraries` and return the file the tool wrote."""
        out = self.root / "{}.bundle".format("-".join(lib.name for lib in libraries))
        gen_bytecode_bundle.build(
            gen_bytecode_bundle.collect(
                [str(lib.manifest) for lib in libraries],
                [str(lib.bytecode_dir) for lib in libraries],
            ),
            str(out),
        )
        return out

    def _bundle(self, *libraries: Library) -> Bundle:
        return self._unpack(self._build(*libraries))

    @staticmethod
    def _unpack(path: Path) -> Bundle:
        data = path.read_bytes()
        magic, pyc_magic_len, index_len = gen_bytecode_bundle.HEADER.unpack_from(data)
        index_start = gen_bytecode_bundle.HEADER.size + pyc_magic_len
        payload_start = index_start + index_len
        return Bundle(
            magic,
            data[gen_bytecode_bundle.HEADER.size : index_start],
            marshal.loads(data[index_start:payload_start]),
            data[payload_start:],
        )


class SourceKeyTest(unittest.TestCase):
    """The inverse of the `get_pyc_path` mapping `compile.py` applies."""

    def test_keys_are_posix_on_every_platform(self) -> None:
        self.assertEqual(
            gen_bytecode_bundle.source_key(
                cache_from_source(os.path.join("a", "b", "c", "d.py"))
            ),
            "a/b/c/d.py",
        )

    def test_a_dotted_file_name_keeps_both_of_its_parts(self) -> None:
        self.assertEqual(
            gen_bytecode_bundle.source_key(
                cache_from_source(os.path.join("a", "b.c.py"))
            ),
            "a/b.c.py",
        )

    def test_bytecode_outside_pycache_is_an_error(self) -> None:
        with self.assertRaises(ValueError):
            gen_bytecode_bundle.source_key(os.path.join("a", "b.cpython-312.pyc"))


class BuildTest(BundleTestBase):
    def test_header_pins_the_format_and_interpreter(self) -> None:
        bundle = self._bundle(self._library("lib", {"pkg/mod.py": "VALUE = 1"}))

        self.assertEqual(bundle.magic, b"PYCBNDL4")
        self.assertEqual(bundle.pyc_magic, MAGIC_NUMBER)

    def test_index_is_keyed_by_path_not_by_module_name(self) -> None:
        # `pkg.py` and `pkg/__init__.py` are the same module name and both
        # stay, because the finder - not the packer - is what decides which one
        # an import resolves to. The top-level `__init__.py` is one
        # rust_make_par synthesizes for a namespace directory; whether it is
        # importable at all is likewise not the packer's business.
        paths = ["top.py", "__init__.py", "pkg.py", "pkg/__init__.py", "pkg/mod.py"]
        bundle = self._bundle(self._library("lib", dict.fromkeys(paths, "VALUE = 1")))

        self.assertEqual(set(bundle.index), set(paths))

    def test_payload_slices_unmarshal_to_runnable_code(self) -> None:
        bundle = self._bundle(self._library("lib", {"pkg/mod.py": "VALUE = 41 + 1"}))

        self.assertEqual(bundle.run("pkg/mod.py")["VALUE"], 42)

    def test_code_keeps_the_tree_relative_filename(self) -> None:
        # `BundleLoader` rewrites this to an absolute path at import time, so a
        # bundle that shipped an absolute build path would leak it into every
        # traceback.
        bundle = self._bundle(self._library("lib", {"pkg/mod.py": "VALUE = 1"}))

        self.assertEqual(bundle.code("pkg/mod.py").co_filename, "pkg/mod.py")

    def test_bundle_is_byte_identical_regardless_of_input_order(self) -> None:
        # The blob is a cached build artifact, so it may not depend on the
        # order buck2 lists the libraries in, nor on the order `compile.py`
        # happened to write entries within one.
        modules = {
            "a.py": "VALUE = 1",
            "b/__init__.py": "VALUE = 2",
            "b/c.py": "VALUE = 3",
        }
        forward = self._library("forward", modules)
        backward = self._library("backward", dict(reversed(list(modules.items()))))
        other = self._library("other", {"d.py": "VALUE = 4"})

        self.assertEqual(
            self._build(forward, other).read_bytes(),
            self._build(other, backward).read_bytes(),
        )

    def test_duplicate_path_keeps_the_first_library(self) -> None:
        # Two libraries can legitimately land the same path in the tree.
        left = self._library("left", {"dup.py": "OWNER = 'left'"})
        right = self._library("right", {"dup.py": "OWNER = 'right'"})

        bundle = self._bundle(left, right)

        self.assertEqual(list(bundle.index), ["dup.py"])
        self.assertEqual(bundle.run("dup.py")["OWNER"], "left")

    def test_the_pyc_path_in_the_manifest_is_never_read(self) -> None:
        # It addresses the action's own output through an `/output_artifacts/`
        # placeholder standing in for a content hash that does not exist while
        # the action runs. The bytes come from the paired directory instead, so
        # a manifest full of nonsense there still bundles correctly.
        library = self._library(
            "lib",
            {"pkg/mod.py": "VALUE = 42"},
            pyc_field="buck-out/v2/gen/fbcode/output_artifacts/aaaaaaaaaaaaaaaa/x",
        )

        self.assertEqual(self._bundle(library).run("pkg/mod.py")["VALUE"], 42)

    def test_rejects_pyc_from_a_different_interpreter(self) -> None:
        library = self._library("lib", {"mod.py": "VALUE = 1"})
        pyc = library.pycs["mod.py"]
        pyc.write_bytes(
            b"\0" * len(MAGIC_NUMBER) + pyc.read_bytes()[len(MAGIC_NUMBER) :]
        )

        with self.assertRaisesRegex(ValueError, "wrong interpreter magic"):
            self._build(library)

    def test_rejects_truncated_pyc(self) -> None:
        library = self._library("lib", {"mod.py": "VALUE = 1"})
        pyc = library.pycs["mod.py"]
        data = pyc.read_bytes()

        for size in (
            gen_bytecode_bundle.PYC_HEADER_LEN - 1,
            gen_bytecode_bundle.PYC_HEADER_LEN,
        ):
            with self.subTest(size=size):
                pyc.write_bytes(data[:size])
                with self.assertRaisesRegex(
                    ValueError, "Truncated .pyc has no bytecode payload"
                ):
                    self._build(library)

    def test_payload_is_streamed_rather_than_buffered(self) -> None:
        # A binary's whole bytecode must never be resident at once.
        blob = "x" * (8 * 1024 * 1024)
        library = self._library("large", {"large.py": f"VALUE = '{blob}'"})

        # `get_traced_memory` reports the whole process, and `start()` is a
        # no-op while tracing is already on, so measure what `_build` adds on
        # top of what is already live rather than the absolute peak.
        was_tracing = tracemalloc.is_tracing()
        tracemalloc.start()
        tracemalloc.reset_peak()
        baseline, _peak = tracemalloc.get_traced_memory()
        try:
            out = self._build(library)
            _current, peak = tracemalloc.get_traced_memory()
        finally:
            if not was_tracing:
                tracemalloc.stop()

        self.assertGreater(out.stat().st_size, len(blob))
        self.assertLess(peak - baseline, len(blob) // 2)


class CollectTest(unittest.TestCase):
    def test_unequal_lengths_are_an_error(self) -> None:
        # Position is the only thing tying the two lists together. If they stop
        # lining up, every pair past the mismatch reads one library's manifest
        # against another library's directory - silently wrong rather than
        # broken, so refuse to build at all.
        for manifests, dirs in ((["unused"], []), ([], ["unused"])):
            with self.subTest(manifests=manifests, dirs=dirs):
                with self.assertRaises(ValueError):
                    gen_bytecode_bundle.collect(manifests, dirs)


class MainTest(BundleTestBase):
    def test_reads_its_inputs_from_files(self) -> None:
        # Large binaries have thousands of libraries, well past the argv limit,
        # so both lists arrive as files.
        libraries = [
            self._library("left", {"pkg/one.py": "VALUE = 1"}),
            self._library("right", {"pkg/two.py": "VALUE = 2"}),
        ]
        (self.root / "manifests.txt").write_text(
            "".join(f"{lib.manifest}\n" for lib in libraries), encoding="utf8"
        )
        (self.root / "artifacts.txt").write_text(
            "".join(f"{lib.bytecode_dir}\n" for lib in libraries), encoding="utf8"
        )
        out = self.root / "bundle.bin"

        gen_bytecode_bundle.main(
            [
                "gen_bytecode_bundle.py",
                f"--output={out}",
                f"--bytecode-manifests-from={self.root / 'manifests.txt'}",
                f"--bytecode-artifacts-from={self.root / 'artifacts.txt'}",
            ]
        )

        self.assertEqual(set(self._unpack(out).index), {"pkg/one.py", "pkg/two.py"})
