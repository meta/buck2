# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Tests for combine_apk_with_relinked_libs.py.

Covers the core replacement logic: flat ZIP .so replacement, nested ZIP/JAR
replacement, error on unreplaced libraries, and the empty-mapping passthrough.
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import struct
import tempfile
import unittest
import zipfile

from android.tools.combine_apk_with_relinked_libs import (
    _build_relinked_lookup,
    _find_matching_key,
    _maybe_rebuild_inner_zip,
    _rewrite_asset_lib_metadata,
    _without_zip64_extra,
    main,
)


def _make_so_bytes(tag: str) -> bytes:
    """Return distinguishable fake .so content."""
    return f"ELF-{tag}".encode()


class BuildRelinkedLookupTest(unittest.TestCase):
    """Tests for _build_relinked_lookup."""

    def test_maps_existing_files(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            abi_dir = os.path.join(d, "arm64-v8a")
            os.makedirs(abi_dir)
            so_path = os.path.join(abi_dir, "libfoo.so")
            with open(so_path, "wb") as f:
                f.write(b"data")

            result = _build_relinked_lookup(["arm64-v8a/libfoo.so"], d)
            self.assertEqual(result, {"arm64-v8a/libfoo.so": so_path})

    def test_raises_on_missing_files(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(RuntimeError):
                _build_relinked_lookup(["arm64-v8a/libmissing.so"], d)

    def test_empty_mapping(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            result = _build_relinked_lookup([], d)
            self.assertEqual(result, {})


class FindMatchingKeyTest(unittest.TestCase):
    """Tests for _find_matching_key."""

    def test_exact_match(self) -> None:
        lookup = {"arm64-v8a/libfoo.so": "/path"}
        self.assertEqual(
            _find_matching_key("arm64-v8a/libfoo.so", lookup), "arm64-v8a/libfoo.so"
        )

    def test_suffix_match(self) -> None:
        lookup = {"arm64-v8a/libfoo.so": "/path"}
        self.assertEqual(
            _find_matching_key("lib/arm64-v8a/libfoo.so", lookup),
            "arm64-v8a/libfoo.so",
        )

    def test_deep_suffix_match(self) -> None:
        lookup = {"arm64-v8a/libfoo.so": "/path"}
        self.assertEqual(
            _find_matching_key("assets/module/lib/arm64-v8a/libfoo.so", lookup),
            "arm64-v8a/libfoo.so",
        )

    def test_no_match(self) -> None:
        lookup = {"arm64-v8a/libfoo.so": "/path"}
        self.assertIsNone(_find_matching_key("arm64-v8a/libbar.so", lookup))

    def test_partial_name_no_false_match(self) -> None:
        lookup = {"arm64-v8a/libfoo.so": "/path"}
        # "xarm64-v8a/libfoo.so" should NOT match because "/" separator is required
        self.assertIsNone(_find_matching_key("xarm64-v8a/libfoo.so", lookup))


class MaybeRebuildInnerZipTest(unittest.TestCase):
    """Tests for _maybe_rebuild_inner_zip."""

    def test_returns_none_when_no_match(self) -> None:
        buf = io.BytesIO()
        with zipfile.ZipFile(buf, "w") as zf:
            zf.writestr("lib/arm64-v8a/libbar.so", b"data")
        lookup = {"arm64-v8a/libfoo.so": "/path"}
        self.assertIsNone(_maybe_rebuild_inner_zip(buf.getvalue(), lookup))

    def test_returns_none_for_bad_zip(self) -> None:
        lookup = {"arm64-v8a/libfoo.so": "/path"}
        self.assertIsNone(_maybe_rebuild_inner_zip(b"not a zip", lookup))

    def test_replaces_matching_entry(self) -> None:
        original = _make_so_bytes("original")
        relinked = _make_so_bytes("relinked")

        buf = io.BytesIO()
        with zipfile.ZipFile(buf, "w") as zf:
            zf.writestr("lib/arm64-v8a/libfoo.so", original)
            zf.writestr("classes.dex", b"dex-data")

        with tempfile.TemporaryDirectory() as d:
            abi_dir = os.path.join(d, "arm64-v8a")
            os.makedirs(abi_dir)
            with open(os.path.join(abi_dir, "libfoo.so"), "wb") as f:
                f.write(relinked)

            lookup = {"arm64-v8a/libfoo.so": os.path.join(abi_dir, "libfoo.so")}
            result = _maybe_rebuild_inner_zip(buf.getvalue(), lookup)

        self.assertIsNotNone(result)
        result_bytes, replaced = result
        self.assertIn("arm64-v8a/libfoo.so", replaced)

        with zipfile.ZipFile(io.BytesIO(result_bytes), "r") as zout:
            self.assertEqual(zout.read("lib/arm64-v8a/libfoo.so"), relinked)
            self.assertEqual(zout.read("classes.dex"), b"dex-data")


class MainIntegrationTest(unittest.TestCase):
    """Integration tests for the main() entry point."""

    def _run_combine(
        self,
        tmpdir: str,
        input_entries: dict[str, bytes],
        mapping: list[str],
        relinked_files: dict[str, bytes],
        existing_input: bool = False,
    ) -> str:
        """Build input APK, mapping JSON, relinked libs dir, run main(), return output path."""
        # Create input APK, unless the test wrote tmpdir/input.apk itself
        input_path = os.path.join(tmpdir, "input.apk")
        if not existing_input:
            with zipfile.ZipFile(input_path, "w") as zf:
                for name, data in input_entries.items():
                    zf.writestr(name, data)

        # Create relinked libs directory
        relinked_dir = os.path.join(tmpdir, "relinked_libs")
        for rel_path, data in relinked_files.items():
            full_path = os.path.join(relinked_dir, rel_path)
            os.makedirs(os.path.dirname(full_path), exist_ok=True)
            with open(full_path, "wb") as f:
                f.write(data)

        # Create mapping JSON
        mapping_path = os.path.join(tmpdir, "mapping.json")
        with open(mapping_path, "w") as f:
            json.dump(mapping, f)

        output_path = os.path.join(tmpdir, "output.apk")

        import sys

        old_argv = sys.argv
        sys.argv = [
            "combine_apk_with_relinked_libs.py",
            "--input",
            input_path,
            "--relinked-libs-dir",
            relinked_dir,
            "--mapping",
            mapping_path,
            "--output",
            output_path,
        ]
        try:
            main()
        finally:
            sys.argv = old_argv

        return output_path

    def test_unchanged_entries_keep_their_compression(self) -> None:
        """Entries that are not replaced keep their compressed bytes. The
        deflated entries use a non-default level, which re-deflating would
        change."""
        payload = b"".join(str(i * 7919 % 10007).encode() for i in range(20000))
        inner = io.BytesIO()
        with zipfile.ZipFile(inner, "w") as jar:
            jar.writestr("classes.dex", payload)

        with tempfile.TemporaryDirectory() as tmpdir:
            input_path = os.path.join(tmpdir, "input.apk")
            with zipfile.ZipFile(input_path, "w") as zf:
                zf.writestr("lib/arm64-v8a/libfoo.so", _make_so_bytes("original"))
                zf.writestr("resources.arsc", payload, compress_type=zipfile.ZIP_STORED)
                zf.writestr(
                    "classes.dex",
                    payload,
                    compress_type=zipfile.ZIP_DEFLATED,
                    compresslevel=1,
                )
                zf.writestr(
                    "assets/module.jar",
                    inner.getvalue(),
                    compress_type=zipfile.ZIP_DEFLATED,
                    compresslevel=1,
                )
            with zipfile.ZipFile(input_path) as zf:
                before = {
                    i.filename: (zf.read(i.filename), i.compress_type, i.compress_size)
                    for i in zf.infolist()
                }

            output = self._run_combine(
                tmpdir,
                input_entries={},
                mapping=["arm64-v8a/libfoo.so"],
                relinked_files={"arm64-v8a/libfoo.so": _make_so_bytes("relinked")},
                existing_input=True,
            )

            with zipfile.ZipFile(output, "r") as zf:
                self.assertIsNone(zf.testzip())
                for name in ("resources.arsc", "classes.dex", "assets/module.jar"):
                    info = zf.getinfo(name)
                    self.assertEqual(
                        (zf.read(name), info.compress_type, info.compress_size),
                        before[name],
                    )

    def test_asset_lib_metadata_matches_replaced_assets(self) -> None:
        """Asset library metadata describes the relinked libraries."""
        unrelinked = _make_so_bytes("unrelinked-and-longer")
        relinked = _make_so_bytes("relinked")

        with tempfile.TemporaryDirectory() as tmpdir:
            output = self._run_combine(
                tmpdir,
                input_entries={
                    "assets/lib/arm64-v8a/libfoo.so": unrelinked,
                    "assets/lib/metadata.txt": _metadata_line(
                        "arm64-v8a/libfoo.so", unrelinked
                    ).encode(),
                },
                mapping=["arm64-v8a/libfoo.so"],
                relinked_files={"arm64-v8a/libfoo.so": relinked},
            )

            with zipfile.ZipFile(output, "r") as zf:
                self.assertEqual(zf.read("assets/lib/arm64-v8a/libfoo.so"), relinked)
                self.assertEqual(
                    zf.read("assets/lib/metadata.txt").decode(),
                    _metadata_line("arm64-v8a/libfoo.so", relinked),
                )

    def test_module_libs_txt_matches_replaced_assets(self) -> None:
        """A Voltron module's libs.txt names its libraries relative to the
        module, as assets/<abi>/<soname> or lib/<abi>/<soname>."""
        unrelinked_a = _make_so_bytes("unrelinked-a-and-longer")
        unrelinked_b = _make_so_bytes("unrelinked-b")
        relinked_a = _make_so_bytes("relinked-a")
        relinked_b = _make_so_bytes("relinked-b-but-longer-still")
        untouched = _make_so_bytes("prebuilt")

        with tempfile.TemporaryDirectory() as tmpdir:
            output = self._run_combine(
                tmpdir,
                input_entries={
                    "assets/mod/assets/arm64-v8a/liba.so": unrelinked_a,
                    "assets/mod/lib/arm64-v8a/libb.so": unrelinked_b,
                    "assets/mod/assets/arm64-v8a/libprebuilt.so": untouched,
                    "assets/mod/assets/libs.txt": "\n".join(
                        [
                            _metadata_line("assets/arm64-v8a/liba.so", unrelinked_a),
                            _metadata_line(
                                "assets/arm64-v8a/libprebuilt.so", untouched
                            ),
                            _metadata_line("lib/arm64-v8a/libb.so", unrelinked_b),
                        ]
                    ).encode(),
                },
                mapping=["arm64-v8a/liba.so", "arm64-v8a/libb.so"],
                relinked_files={
                    "arm64-v8a/liba.so": relinked_a,
                    "arm64-v8a/libb.so": relinked_b,
                },
            )

            with zipfile.ZipFile(output, "r") as zf:
                self.assertEqual(
                    zf.read("assets/mod/assets/libs.txt").decode(),
                    "\n".join(
                        [
                            _metadata_line("lib/arm64-v8a/libb.so", relinked_b),
                            _metadata_line("assets/arm64-v8a/liba.so", relinked_a),
                            _metadata_line(
                                "assets/arm64-v8a/libprebuilt.so", untouched
                            ),
                        ]
                    ),
                )

    def test_flat_so_replacement(self) -> None:
        """Standard lib/<abi>/<soname> entries are replaced."""
        original = _make_so_bytes("original")
        relinked = _make_so_bytes("relinked")

        with tempfile.TemporaryDirectory() as tmpdir:
            output = self._run_combine(
                tmpdir,
                input_entries={
                    "lib/arm64-v8a/libfoo.so": original,
                    "classes.dex": b"dex-data",
                },
                mapping=["arm64-v8a/libfoo.so"],
                relinked_files={"arm64-v8a/libfoo.so": relinked},
            )

            with zipfile.ZipFile(output, "r") as zf:
                self.assertEqual(zf.read("lib/arm64-v8a/libfoo.so"), relinked)
                self.assertEqual(zf.read("classes.dex"), b"dex-data")

    def test_nested_jar_replacement(self) -> None:
        """Libraries inside nested JARs are replaced."""
        original = _make_so_bytes("original")
        relinked = _make_so_bytes("relinked")

        inner_jar = io.BytesIO()
        with zipfile.ZipFile(inner_jar, "w") as zf:
            zf.writestr("lib/arm64-v8a/libfoo.so", original)

        with tempfile.TemporaryDirectory() as tmpdir:
            output = self._run_combine(
                tmpdir,
                input_entries={
                    "module.jar": inner_jar.getvalue(),
                    "classes.dex": b"dex-data",
                },
                mapping=["arm64-v8a/libfoo.so"],
                relinked_files={"arm64-v8a/libfoo.so": relinked},
            )

            with zipfile.ZipFile(output, "r") as zf:
                inner_bytes = zf.read("module.jar")
                with zipfile.ZipFile(io.BytesIO(inner_bytes), "r") as inner:
                    self.assertEqual(inner.read("lib/arm64-v8a/libfoo.so"), relinked)

    def test_empty_mapping_copies_input(self) -> None:
        """When the mapping is empty, the output should be a copy of the input."""
        with tempfile.TemporaryDirectory() as tmpdir:
            input_path = os.path.join(tmpdir, "input.apk")
            with zipfile.ZipFile(input_path, "w") as zf:
                zf.writestr("classes.dex", b"dex-data")

            mapping_path = os.path.join(tmpdir, "mapping.json")
            with open(mapping_path, "w") as f:
                json.dump([], f)

            # Create an empty relinked dir
            relinked_dir = os.path.join(tmpdir, "relinked_libs")
            os.makedirs(relinked_dir)

            output_path = os.path.join(tmpdir, "output.apk")

            import sys

            old_argv = sys.argv
            sys.argv = [
                "combine_apk_with_relinked_libs.py",
                "--input",
                input_path,
                "--relinked-libs-dir",
                relinked_dir,
                "--mapping",
                mapping_path,
                "--output",
                output_path,
            ]
            try:
                main()
            finally:
                sys.argv = old_argv

            with zipfile.ZipFile(output_path, "r") as zf:
                self.assertEqual(zf.read("classes.dex"), b"dex-data")

    def test_unreplaced_library_raises(self) -> None:
        """If a mapped library is not found in the input APK, a RuntimeError is raised."""
        with tempfile.TemporaryDirectory() as tmpdir:
            with self.assertRaises(RuntimeError):
                self._run_combine(
                    tmpdir,
                    input_entries={
                        "classes.dex": b"dex-data",
                    },
                    mapping=["arm64-v8a/libfoo.so"],
                    relinked_files={"arm64-v8a/libfoo.so": b"relinked"},
                )

    def test_identical_copies_are_all_replaced(self) -> None:
        """Copies of one library at several locations, nested jars included,
        are all replaced."""
        original = _make_so_bytes("original")
        relinked = _make_so_bytes("relinked")
        inner_jar = io.BytesIO()
        with zipfile.ZipFile(inner_jar, "w") as zf:
            zf.writestr("lib/arm64-v8a/libfoo.so", original)

        with tempfile.TemporaryDirectory() as tmpdir:
            output = self._run_combine(
                tmpdir,
                input_entries={
                    "lib/arm64-v8a/libfoo.so": original,
                    "assets/lib/arm64-v8a/libfoo.so": original,
                    "module.jar": inner_jar.getvalue(),
                },
                mapping=["arm64-v8a/libfoo.so"],
                relinked_files={"arm64-v8a/libfoo.so": relinked},
            )
            with zipfile.ZipFile(output, "r") as zf:
                self.assertEqual(zf.read("lib/arm64-v8a/libfoo.so"), relinked)
                self.assertEqual(zf.read("assets/lib/arm64-v8a/libfoo.so"), relinked)
                with zipfile.ZipFile(io.BytesIO(zf.read("module.jar"))) as inner:
                    self.assertEqual(inner.read("lib/arm64-v8a/libfoo.so"), relinked)

    def test_replaced_entries_keep_their_compression_method(self) -> None:
        """A replaced library keeps the compression method of the entry it
        replaces: stored in lib/, deflated where the input deflated it."""
        original = _make_so_bytes("original")
        relinked = _make_so_bytes("relinked")
        with tempfile.TemporaryDirectory() as tmpdir:
            input_path = os.path.join(tmpdir, "input.apk")
            with zipfile.ZipFile(input_path, "w") as zf:
                zf.writestr(
                    "lib/arm64-v8a/libfoo.so",
                    original,
                    compress_type=zipfile.ZIP_STORED,
                )
                zf.writestr(
                    "assets/lib/arm64-v8a/libbar.so",
                    original + b"bar",
                    compress_type=zipfile.ZIP_DEFLATED,
                )
            output = self._run_combine(
                tmpdir,
                input_entries={},
                mapping=["arm64-v8a/libfoo.so", "arm64-v8a/libbar.so"],
                relinked_files={
                    "arm64-v8a/libfoo.so": relinked,
                    "arm64-v8a/libbar.so": relinked + b"bar",
                },
                existing_input=True,
            )
            with zipfile.ZipFile(output, "r") as zf:
                self.assertEqual(
                    zf.getinfo("lib/arm64-v8a/libfoo.so").compress_type,
                    zipfile.ZIP_STORED,
                )
                self.assertEqual(
                    zf.getinfo("assets/lib/arm64-v8a/libbar.so").compress_type,
                    zipfile.ZIP_DEFLATED,
                )

    def test_different_libraries_under_one_soname_raise(self) -> None:
        """A prebuilt and a relinked library packaged under one <abi>/<soname>
        cannot both be replaced, in the outer APK or in a nested jar."""
        relinked = _make_so_bytes("relinked")
        inner_jar = io.BytesIO()
        with zipfile.ZipFile(inner_jar, "w") as zf:
            zf.writestr("lib/arm64-v8a/libfoo.so", _make_so_bytes("prebuilt"))
        for other in [
            {"assets/prebuilt/arm64-v8a/libfoo.so": _make_so_bytes("prebuilt")},
            {"module.jar": inner_jar.getvalue()},
        ]:
            with tempfile.TemporaryDirectory() as tmpdir:
                with self.assertRaisesRegex(RuntimeError, "arm64-v8a/libfoo.so"):
                    self._run_combine(
                        tmpdir,
                        input_entries={
                            "lib/arm64-v8a/libfoo.so": _make_so_bytes("original"),
                            **other,
                        },
                        mapping=["arm64-v8a/libfoo.so"],
                        relinked_files={"arm64-v8a/libfoo.so": relinked},
                    )


def _metadata_line(path: str, data: bytes) -> str:
    return "{} {} {}".format(path, len(data), hashlib.sha256(data).hexdigest())


class RewriteAssetLibMetadataTest(unittest.TestCase):
    """Tests for _rewrite_asset_lib_metadata."""

    def test_updates_replaced_entries_and_resorts(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            relinked = os.path.join(d, "libsmall.so")
            relinked_data = b"x" * 50
            with open(relinked, "wb") as f:
                f.write(relinked_data)
            big = b"y" * 20
            small = b"z" * 10
            content = "\n".join(
                [
                    _metadata_line("arm64-v8a/libother.so", big),
                    _metadata_line("arm64-v8a/libsmall.so", small),
                ]
            ).encode()

            result = _rewrite_asset_lib_metadata(
                content, {"arm64-v8a/libsmall.so": relinked}
            )

            self.assertEqual(
                result,
                "\n".join(
                    [
                        _metadata_line("arm64-v8a/libsmall.so", relinked_data),
                        _metadata_line("arm64-v8a/libother.so", big),
                    ]
                ).encode(),
            )

    def test_keeps_a_trailing_newline(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            relinked = os.path.join(d, "libfoo.so")
            relinked_data = b"x" * 50
            with open(relinked, "wb") as f:
                f.write(relinked_data)
            content = (_metadata_line("arm64-v8a/libfoo.so", b"z" * 10) + "\n").encode()

            result = _rewrite_asset_lib_metadata(
                content, {"arm64-v8a/libfoo.so": relinked}
            )

            self.assertEqual(
                result,
                (_metadata_line("arm64-v8a/libfoo.so", relinked_data) + "\n").encode(),
            )

    def test_leaves_unrelated_metadata_alone(self) -> None:
        content = b"secondary-1.dex.jar 1234 deadbeef\n"
        self.assertIsNone(
            _rewrite_asset_lib_metadata(content, {"arm64-v8a/libfoo.so": "/path"})
        )

    def test_leaves_binary_content_alone(self) -> None:
        self.assertIsNone(
            _rewrite_asset_lib_metadata(b"\xff\xfe", {"arm64-v8a/libfoo.so": "/x"})
        )

    def test_none_when_no_entry_is_replaced(self) -> None:
        content = _metadata_line("arm64-v8a/libfoo.so", b"abc").encode()
        self.assertIsNone(
            _rewrite_asset_lib_metadata(content, {"arm64-v8a/libbar.so": "/path"})
        )


class WithoutZip64ExtraTest(unittest.TestCase):
    """Tests for _without_zip64_extra."""

    def test_drops_only_the_zip64_field(self) -> None:
        zip64 = struct.pack("<HHQQ", 0x0001, 16, 1 << 33, 1 << 33)
        padding = struct.pack("<HH", 0xD935, 2) + b"\0\0"
        self.assertEqual(_without_zip64_extra(padding + zip64 + padding), padding * 2)

    def test_empty(self) -> None:
        self.assertEqual(_without_zip64_extra(b""), b"")
