# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import contextlib
import hashlib
import io
import os
import random
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile
import zlib
from collections.abc import Iterator
from pathlib import Path
from unittest import mock

from package_tree import compress, main, write_archive

LONG = "l" * 99
TREE: dict[str, tuple[bytes, bool] | None] = {
    "package.json": (b'{"name": "app"}', False),
    "bin/app": (b"#!/bin/sh\n", True),
    "bin/app.cmd": (b"@echo off\r\n", False),
    "node": (bytes(range(256)) * 64, True),
    "node_modules/@s/x/package.json": (b"{}", False),
    "node_modules/a/index.js": (b"module.exports = 'a';\n", False),
    "node_modules/a-b/index.js": (b"module.exports = 'a-b';\n", False),
    "node_modules/a.js": (b"", False),
    "café.js": ("// café\n".encode(), False),
    # Its tar names exceed the 100 characters of a ustar name field.
    f"{LONG}/f.js": (b"long", False),
    "empty": None,
}
ORDER = [
    "app/",
    "app/bin/",
    "app/bin/app",
    "app/bin/app.cmd",
    "app/café.js",
    "app/empty/",
    f"app/{LONG}/",
    f"app/{LONG}/f.js",
    "app/node",
    "app/node_modules/",
    "app/node_modules/@s/",
    "app/node_modules/@s/x/",
    "app/node_modules/@s/x/package.json",
    "app/node_modules/a/",
    "app/node_modules/a/index.js",
    "app/node_modules/a-b/",
    "app/node_modules/a-b/index.js",
    "app/node_modules/a.js",
    "app/package.json",
]
TAR_MTIME = 499162500
EXECUTABLE = ("app/bin/app", "app/node")

# Stands in for `zstd -o OUT IN`, checking that the flags it was given come first.
FAKE_ZSTD = """\
import sys
*flags, o, out, src = sys.argv[1:]
assert flags == ["--level", "7"] and o == "-o", sys.argv
with open(src, "rb") as f, open(out, "wb") as g:
    g.write(b"FAKE" + f.read())
"""

_SCANDIR = os.scandir


class ReversedScandir:
    def __init__(self, path: object) -> None:
        with _SCANDIR(path) as entries:
            self.entries = list(entries)[::-1]

    def __iter__(self) -> Iterator[os.DirEntry[str]]:
        return iter(self.entries)

    def __enter__(self) -> ReversedScandir:
        return self

    def __exit__(self, *exc_info: object) -> None:
        pass


def make_tree(root: Path, seed: int, umask: int) -> Path:
    """Writes TREE to `root`, creating paths in an order and with mtimes that depend on `seed`."""
    rng = random.Random(seed)
    paths = list(TREE)
    rng.shuffle(paths)
    old_umask = os.umask(umask)
    try:
        for rel in paths:
            path = root / rel
            if TREE[rel] is None:
                path.mkdir(parents=True, exist_ok=True)
                continue
            contents, executable = TREE[rel]
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(contents)
            if executable:
                path.chmod(0o777 & ~umask)
    finally:
        os.umask(old_umask)
    for dirpath, dirnames, filenames in os.walk(root, topdown=False):
        for name in [*filenames, *dirnames]:
            mtime = rng.randrange(1 << 31)
            os.utime(os.path.join(dirpath, name), (mtime, mtime))
    return root


def tree_listing(src: Path, root: str) -> list[str]:
    """The names an archive of `src` under `root` would give its directories and files."""
    names = [root + "/"]
    for dirpath, dirnames, filenames in os.walk(src):
        rel = Path(dirpath).relative_to(src).as_posix()
        base = root + "/" + ("" if rel == "." else rel + "/")
        names += [base + name + "/" for name in dirnames]
        names += [base + name for name in filenames]
    return sorted(names)


def deflate(data: bytes, level: int) -> bytes:
    compressor = zlib.compressobj(level, zlib.DEFLATED, -zlib.MAX_WBITS)
    return compressor.compress(data) + compressor.flush()


def file_modes(data: bytes, archive_format: str) -> dict[str, int]:
    if archive_format == "zip":
        with zipfile.ZipFile(io.BytesIO(data)) as z:
            return {
                i.filename: (i.external_attr >> 16) & 0o7777
                for i in z.infolist()
                if not i.is_dir()
            }
    with tarfile.open(fileobj=io.BytesIO(data)) as t:
        return {m.name: m.mode for m in t.getmembers() if m.isreg()}


class TestArchive(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(os.path.realpath(tmp.name))
        self.fake_zstd = self.tmp / "fake_zstd.py"
        self.fake_zstd.write_text(FAKE_ZSTD)

    def _archive(self, src: Path, archive_format: str, **kwargs: object) -> bytes:
        out = Path(tempfile.mkdtemp(dir=self.tmp)) / "out"
        written = ["out"]
        if archive_format == "tar.zst":
            zstd = kwargs.pop(
                "zstd", [sys.executable, str(self.fake_zstd), "--level", "7"]
            )
            tar = out.with_name("out.tar")
            write_archive(src, "tar", tar, **kwargs)
            compress(tar, out, zstd)
            written.append(tar.name)
        else:
            write_archive(src, archive_format, out, **kwargs)
        self.assertEqual(sorted(os.listdir(out.parent)), written)
        return out.read_bytes()

    def _fail(self, argv: list[str]) -> str:
        stderr = io.StringIO()
        with mock.patch.object(sys, "argv", argv):
            with contextlib.redirect_stderr(stderr):
                self.assertEqual(main(), 1)
        return stderr.getvalue()

    def assert_extracted(self, dest: Path, src: Path) -> None:
        self.assertEqual(tree_listing(dest, "app"), tree_listing(src, "app"))
        for dirpath, _, filenames in os.walk(src):
            for name in filenames:
                path = Path(dirpath, name)
                extracted = dest / path.relative_to(src)
                self.assertEqual(extracted.read_bytes(), path.read_bytes())
                if sys.platform != "win32":
                    executable = path.stat().st_mode & stat.S_IXUSR
                    self.assertEqual(
                        stat.S_IMODE(extracted.stat().st_mode),
                        0o755 if executable else 0o644,
                        extracted,
                    )

    def test_bytes_ignore_mtimes_umask_order_and_location(self) -> None:
        trees = [
            make_tree(self.tmp / "one" / "app", seed=1, umask=0o022),
            make_tree(self.tmp / "two" / "an other", seed=2, umask=0o077),
        ]
        for archive_format in ("zip", "tar", "tar.gz", "tar.zst"):
            with self.subTest(archive_format):
                first = self._archive(trees[0], archive_format, root="app")
                with mock.patch("os.scandir", ReversedScandir):
                    second = self._archive(trees[1], archive_format, root="app")
                self.assertEqual(first, second)

    def test_order(self) -> None:
        tree = make_tree(self.tmp / "tree", seed=1, umask=0o022)
        with zipfile.ZipFile(io.BytesIO(self._archive(tree, "zip", root="app"))) as z:
            self.assertEqual(z.namelist(), ORDER)
        with tarfile.open(
            fileobj=io.BytesIO(self._archive(tree, "tar", root="app"))
        ) as t:
            self.assertEqual(t.getnames(), [name.rstrip("/") for name in ORDER])

    def test_zip_entries(self) -> None:
        tree = make_tree(self.tmp / "tree", seed=1, umask=0o077)
        with zipfile.ZipFile(io.BytesIO(self._archive(tree, "zip", root="app"))) as z:
            self.assertEqual(sorted(z.namelist()), tree_listing(tree, "app"))
            for info in z.infolist():
                with self.subTest(info.filename):
                    self.assertEqual(info.date_time, (1985, 2, 1, 0, 0, 0))
                    self.assertEqual(info.create_system, 3)
                    if info.is_dir():
                        self.assertEqual(info.external_attr, 0o40755 << 16 | 0x10)
                        self.assertEqual(info.compress_type, zipfile.ZIP_STORED)
                        continue
                    mode = 0o755 if info.filename in EXECUTABLE else 0o644
                    if sys.platform != "win32":
                        self.assertEqual(info.external_attr, (0o100000 | mode) << 16)
                    self.assertEqual(info.compress_type, zipfile.ZIP_DEFLATED)
                    self.assertEqual(
                        z.read(info), TREE[info.filename.removeprefix("app/")][0]
                    )

    def test_zip_deflate_level(self) -> None:
        rng = random.Random(0)
        words = ["module", "exports", "require", "function", "=>", "{", "}", "\n"]
        data = " ".join(rng.choice(words) for _ in range(20000)).encode()
        tree = self.tmp / "tree"
        tree.mkdir()
        (tree / "index.js").write_bytes(data)
        with zipfile.ZipFile(io.BytesIO(self._archive(tree, "zip"))) as z:
            size = z.getinfo("index.js").compress_size
        self.assertEqual(size, len(deflate(data, 9)))
        # The data tells the default level from level 9.
        self.assertNotEqual(size, len(deflate(data, 6)))

    def test_executables(self) -> None:
        tree = make_tree(self.tmp / "tree", seed=1, umask=0o022)
        listed = ["package.json", "node_modules/@s/x/package.json"]
        executable = set(listed)
        if sys.platform != "win32":
            executable.update(name.removeprefix("app/") for name in EXECUTABLE)
        files = sorted(path for path, file in TREE.items() if file is not None)
        for archive_format in ("zip", "tar"):
            with self.subTest(archive_format):
                data = self._archive(tree, archive_format, executables=listed)
                modes = file_modes(data, archive_format)
                self.assertEqual(sorted(modes), files)
                self.assertEqual(
                    modes,
                    {name: 0o755 if name in executable else 0o644 for name in files},
                )

    @unittest.skipIf(sys.platform == "win32", "compares with an OS with execute bits")
    def test_windows_takes_execute_bits_from_the_list(self) -> None:
        tree = make_tree(self.tmp / "tree", seed=1, umask=0o022)
        executables = [name.removeprefix("app/") for name in EXECUTABLE]
        for archive_format in ("zip", "tar"):
            with self.subTest(archive_format):
                data = self._archive(tree, archive_format, root="app")
                # zipfile also marks entries as MS-DOS ones by default on Windows.
                with mock.patch.object(sys, "platform", "win32"):
                    unlisted = self._archive(tree, archive_format, root="app")
                    listed = self._archive(
                        tree, archive_format, root="app", executables=executables
                    )
                self.assertEqual(
                    set(file_modes(unlisted, archive_format).values()), {0o644}
                )
                self.assertEqual(listed, data)

    def test_rejects_executables_that_are_not_files(self) -> None:
        tree = make_tree(self.tmp / "tree", seed=1, umask=0o022)
        for listed in ("bin/missing", "bin", "./node", "app/node"):
            for archive_format in ("zip", "tar"):
                with self.subTest(listed=listed, format=archive_format):
                    out = Path(tempfile.mkdtemp(dir=self.tmp)) / "out"
                    error = f"executable {listed} names no file in the directory"
                    with self.assertRaisesRegex(ValueError, re.escape(error)):
                        write_archive(
                            tree, archive_format, out, root="app", executables=[listed]
                        )
                    self.assertFalse(out.exists())

    def test_tar_entries(self) -> None:
        tree = make_tree(self.tmp / "tree", seed=1, umask=0o077)
        os.link(tree / "package.json", tree / "hard.json")
        data = self._archive(tree, "tar", root="app")
        # POSIX, not GNU, headers.
        self.assertEqual(data[257:265], b"ustar\x0000")
        with tarfile.open(fileobj=io.BytesIO(data)) as t:
            members = t.getmembers()
            for member in members:
                with self.subTest(member.name):
                    self.assertEqual(member.mtime, TAR_MTIME)
                    self.assertEqual((member.uid, member.gid), (0, 0))
                    self.assertEqual((member.uname, member.gname), ("", ""))
                    if member.isdir():
                        self.assertEqual(member.mode, 0o755)
                        continue
                    self.assertTrue(member.isreg())
                    if sys.platform != "win32":
                        executable = member.name in EXECUTABLE
                        self.assertEqual(member.mode, 0o755 if executable else 0o644)
            self.assertEqual(t.extractfile("app/hard.json").read(), b'{"name": "app"}')
        long_names = {m.name: m.pax_headers for m in members if LONG in m.name}
        self.assertEqual(
            long_names,
            {
                f"app/{LONG}": {"path": f"app/{LONG}/"},
                f"app/{LONG}/f.js": {"path": f"app/{LONG}/f.js"},
            },
        )

    def test_gzip_header(self) -> None:
        tree = make_tree(self.tmp / "tree", seed=1, umask=0o022)
        data = self._archive(tree, "tar.gz")
        # No FNAME flag, a zero mtime, the best-compression flag and an unknown OS.
        self.assertEqual(data[:10], b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x02\xff")

    def test_tar_gz_round_trip(self) -> None:
        tree = make_tree(self.tmp / "tree", seed=1, umask=0o077)
        out = self.tmp / "app.tar.gz"
        out.write_bytes(self._archive(tree, "tar.gz", root="app"))
        with tarfile.open(out) as t:
            t.extractall(self.tmp / "x", filter="data")
        self.assert_extracted(self.tmp / "x" / "app", tree)

    @unittest.skipUnless(shutil.which("unzip"), "unzip is not installed")
    def test_zip_round_trip(self) -> None:
        tree = make_tree(self.tmp / "tree", seed=1, umask=0o077)
        out = self.tmp / "app.zip"
        out.write_bytes(self._archive(tree, "zip", root="app"))
        umask = os.umask(0o022)
        try:
            subprocess.run(
                ["unzip", "-q", str(out), "-d", str(self.tmp / "x")], check=True
            )
        finally:
            os.umask(umask)
        self.assert_extracted(self.tmp / "x" / "app", tree)

    @unittest.skipUnless(shutil.which("zstd"), "zstd is not installed")
    def test_tar_zst_round_trip(self) -> None:
        zstd = [shutil.which("zstd"), "-q", "-19", "-T1"]
        tree = make_tree(self.tmp / "tree", seed=1, umask=0o022)
        first = self._archive(tree, "tar.zst", root="app", zstd=zstd)
        self.assertEqual(first, self._archive(tree, "tar.zst", root="app", zstd=zstd))
        decompressed = subprocess.run(
            [zstd[0], "-d", "-q", "-c"], input=first, capture_output=True, check=True
        ).stdout
        self.assertEqual(decompressed, self._archive(tree, "tar", root="app"))

    def test_zstd_failure(self) -> None:
        tar = self.tmp / "app.tar"
        tar.write_bytes(b"tar")
        zstd = [sys.executable, "-c", "import sys; sys.exit(3)"]
        with self.assertRaises(subprocess.CalledProcessError) as e:
            compress(tar, self.tmp / "out", zstd, self.tmp / "out.sha256")
        self.assertEqual(e.exception.returncode, 3)
        self.assertEqual(sorted(os.listdir(self.tmp)), ["app.tar", "fake_zstd.py"])

    def test_zstd_failure_removes_partial_output(self) -> None:
        tar = self.tmp / "app.tar"
        tar.write_bytes(b"tar")
        out = self.tmp / "out"
        partial = f"open({str(out)!r}, 'wb').write(b'part'); raise SystemExit(1)"
        with self.assertRaises(subprocess.CalledProcessError):
            compress(tar, out, [sys.executable, "-c", partial])
        self.assertFalse(out.exists())

    def test_failed_write_removes_partial_output(self) -> None:
        tree = make_tree(self.tmp / "tree", seed=1, umask=0o022)
        for archive_format, cls, method in (
            ("zip", zipfile.ZipFile, "writestr"),
            ("tar", tarfile.TarFile, "addfile"),
            ("tar.gz", tarfile.TarFile, "addfile"),
        ):
            with self.subTest(archive_format):
                real = getattr(cls, method)
                calls = iter(range(2))

                def write_twice(
                    *args: object, real=real, calls=calls, **kwargs: object
                ) -> object:
                    if next(calls, None) is None:
                        raise OSError(28, "No space left on device")
                    return real(*args, **kwargs)

                out = self.tmp / f"out.{archive_format}"
                with mock.patch.object(cls, method, write_twice):
                    with self.assertRaisesRegex(OSError, "No space left"):
                        write_archive(tree, archive_format, out)
                self.assertFalse(out.exists())

    def test_failed_sha256_write_removes_outputs(self) -> None:
        tree = make_tree(self.tmp / "tree", seed=1, umask=0o022)
        tar = self.tmp / "app.tar"
        tar.write_bytes(b"tar")
        copy = [
            sys.executable,
            "-c",
            "import shutil, sys; shutil.copy(sys.argv[3], sys.argv[2])",
        ]

        def write_partly(path: Path, data: bytes) -> int:
            with open(path, "wb") as f:
                f.write(data[:3])
            raise OSError(28, "No space left on device")

        for name, write in (
            (
                "archive",
                lambda out, sha256: write_archive(tree, "tar", out, sha256=sha256),
            ),
            ("compress", lambda out, sha256: compress(tar, out, copy, sha256)),
        ):
            with self.subTest(name):
                out, sha256 = self.tmp / f"{name}.out", self.tmp / f"{name}.sha256"
                with mock.patch.object(Path, "write_bytes", write_partly):
                    with self.assertRaisesRegex(OSError, "No space left"):
                        write(out, sha256)
                self.assertFalse(out.exists())
                self.assertFalse(sha256.exists())

    @unittest.skipIf(sys.platform == "win32", "Windows has no owner execute bit")
    def test_modes(self) -> None:
        tree = self.tmp / "tree"
        tree.mkdir()
        modes = {
            "read-only": (0o444, 0o644),
            "private": (0o600, 0o644),
            "others execute": (0o611, 0o644),
            "owner execute": (0o700, 0o755),
            "setuid": (0o4755, 0o755),
        }
        for name, (mode, _) in modes.items():
            (tree / name).write_bytes(b"x")
            (tree / name).chmod(mode)
        with zipfile.ZipFile(io.BytesIO(self._archive(tree, "zip"))) as z:
            self.assertEqual(
                {i.filename: i.external_attr >> 16 for i in z.infolist()},
                {name: 0o100000 | want for name, (_, want) in modes.items()},
            )
        with tarfile.open(fileobj=io.BytesIO(self._archive(tree, "tar"))) as t:
            self.assertEqual(
                {m.name: m.mode for m in t.getmembers()},
                {name: want for name, (_, want) in modes.items()},
            )

    def test_root(self) -> None:
        tree = self.tmp / "tree"
        (tree / "d").mkdir(parents=True)
        (tree / "d" / "f").write_bytes(b"f")
        for root, names in (
            ("", ["d/", "d/f"]),
            ("app", ["app/", "app/d/", "app/d/f"]),
            ("a b/c", ["a b/", "a b/c/", "a b/c/d/", "a b/c/d/f"]),
        ):
            with self.subTest(root):
                data = self._archive(tree, "zip", root=root)
                with zipfile.ZipFile(io.BytesIO(data)) as z:
                    self.assertEqual(z.namelist(), names)
                with tarfile.open(
                    fileobj=io.BytesIO(self._archive(tree, "tar", root=root))
                ) as t:
                    self.assertEqual(t.getnames(), [n.rstrip("/") for n in names])

    def test_rejects(self) -> None:
        tree = self.tmp / "tree"
        tree.mkdir()
        for case, args, error in (
            ("parent root", {"root": "../app"}, "portable"),
            ("absolute root", {"root": "/app"}, "portable"),
            ("empty root component", {"root": "a//b"}, "portable"),
            ("trailing slash", {"root": "app/"}, "portable"),
            ("backslash root", {"root": "a\\b"}, "portable"),
            ("device root", {"root": "aux"}, "portable"),
            ("control character", {"root": "a\nb"}, "portable"),
            (
                "tar.zst, which compress writes",
                {"archive_format": "tar.zst"},
                "unknown archive format",
            ),
        ):
            with self.subTest(case):
                out = Path(tempfile.mkdtemp(dir=self.tmp)) / "out"
                args = {"archive_format": "zip", **args}
                with self.assertRaisesRegex(ValueError, error):
                    write_archive(tree, out=out, **args)
                self.assertFalse(out.exists())

    @unittest.skipUnless(hasattr(os, "mkfifo"), "needs os.mkfifo")
    def test_rejects_special_files(self) -> None:
        tree = self.tmp / "tree"
        (tree / "d").mkdir(parents=True)
        os.mkfifo(tree / "d" / "fifo")
        for archive_format in ("zip", "tar"):
            with self.subTest(archive_format):
                out = self.tmp / archive_format
                with self.assertRaisesRegex(
                    ValueError, "d/fifo is not a file, directory or symlink"
                ):
                    write_archive(tree, archive_format, out)
                self.assertFalse(out.exists())

    def test_archive_cli(self) -> None:
        tree = make_tree(self.tmp / "tree", seed=1, umask=0o022)
        executables = self.tmp / "executables"
        executables.write_text("package.json\n \n  node_modules/a/index.js \n")
        out, sha256 = self.tmp / "app.zip", self.tmp / "app.zip.sha256"
        argv = [
            "package_tree.py",
            "archive",
            "--src",
            str(tree),
            "--format",
            "zip",
            "--out",
            str(out),
            "--root=app",
            "--executables",
            str(executables),
            "--sha256",
            str(sha256),
        ]
        with mock.patch.object(sys, "argv", argv):
            self.assertEqual(main(), 0)
        data = self._archive(
            tree,
            "zip",
            root="app",
            executables=["package.json", "node_modules/a/index.js"],
        )
        self.assertEqual(out.read_bytes(), data)
        self.assertEqual(
            sha256.read_bytes(), hashlib.sha256(data).hexdigest().encode() + b"\n"
        )
        executables.write_text("node_modules\n")
        for case, args, error in (
            ("bad root", [*argv[:8], "--root=..", *argv[9:]], "root '..' is not"),
            ("not a file", argv, "executable node_modules names no file"),
            (
                "no executables file",
                [*argv[:-3], str(self.tmp / "missing"), *argv[-2:]],
                "[Errno 2]",
            ),
        ):
            with self.subTest(case):
                self.assertTrue(self._fail(args).startswith(f"archive: {error}"))

    def test_compress_cli(self) -> None:
        tar = self.tmp / "app.tar"
        tar.write_bytes(b"tar")
        out, sha256 = self.tmp / "app.tar.zst", self.tmp / "app.tar.zst.sha256"
        argv = [
            "package_tree.py",
            "compress",
            "--src",
            str(tar),
            "--out",
            str(out),
            "--sha256",
            str(sha256),
            "--zstd",
            sys.executable,
            str(self.fake_zstd),
            "--level",
            "7",
        ]
        with mock.patch.object(sys, "argv", argv):
            self.assertEqual(main(), 0)
        self.assertEqual(out.read_bytes(), b"FAKEtar")
        digest = hashlib.sha256(b"FAKEtar").hexdigest()
        self.assertEqual(sha256.read_bytes(), digest.encode() + b"\n")
        out.unlink()
        sha256.unlink()
        missing = str(self.tmp / "missing")
        with self.assertRaises(FileNotFoundError) as not_found:
            subprocess.run([missing])
        for case, args, error in (
            ("no zstd command", argv[:-4], "no zstd command"),
            ("zstd fails", [*argv[:-3], "-c", "import sys; sys.exit(3)"], "status 3"),
            ("no such zstd", [*argv[:-4], missing], str(not_found.exception)),
        ):
            with self.subTest(case):
                printed = self._fail(args)
                self.assertTrue(printed.startswith("compress: "), printed)
                self.assertIn(error, printed)
                self.assertEqual(printed.count("\n"), 1, printed)
                self.assertFalse(out.exists() or sha256.exists())


@unittest.skipIf(sys.platform == "win32", "creating symlinks needs privileges")
class TestArchiveSymlinks(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(os.path.realpath(tmp.name))
        self.tree = self.tmp / "tree"
        (self.tree / "pkg" / "lib").mkdir(parents=True)
        (self.tree / "bin").mkdir()
        (self.tree / "pkg" / "cli.js").write_bytes(b"cli")
        (self.tree / "pkg" / "cli.js").chmod(0o755)
        (self.tree / "pkg" / "lib" / "x.js").write_bytes(b"x")
        os.symlink("../pkg/cli.js", self.tree / "bin" / "cli")
        os.symlink("pkg/lib", self.tree / "lib", target_is_directory=True)

    def test_zip_stores_what_symlinks_point_to(self) -> None:
        write_archive(self.tree, "zip", self.tmp / "out.zip")
        with zipfile.ZipFile(self.tmp / "out.zip") as z:
            self.assertEqual(
                z.namelist(),
                [
                    "bin/",
                    "bin/cli",
                    "lib/",
                    "lib/x.js",
                    "pkg/",
                    "pkg/cli.js",
                    "pkg/lib/",
                    "pkg/lib/x.js",
                ],
            )
            self.assertEqual(z.read("bin/cli"), b"cli")
            self.assertEqual(z.getinfo("bin/cli").external_attr >> 16, 0o100755)
            self.assertEqual(z.read("lib/x.js"), b"x")

    def test_tar_stores_symlinks(self) -> None:
        write_archive(self.tree, "tar", self.tmp / "out.tar")
        with tarfile.open(self.tmp / "out.tar") as t:
            links = {m.name: (m.linkname, m.mode) for m in t.getmembers() if m.issym()}
            self.assertEqual(
                links, {"bin/cli": ("../pkg/cli.js", 0o777), "lib": ("pkg/lib", 0o777)}
            )
            t.extractall(self.tmp / "x", filter="data")
        self.assertEqual((self.tmp / "x" / "bin" / "cli").read_bytes(), b"cli")
        self.assertEqual((self.tmp / "x" / "lib" / "x.js").read_bytes(), b"x")

    def test_symlinks_are_checked_where_they_are(self) -> None:
        # zip also reaches `up` as lib/up, where `../../bin` would climb above the directory.
        os.symlink("../../bin", self.tree / "pkg" / "lib" / "up")
        write_archive(self.tree, "zip", self.tmp / "out.zip")
        with zipfile.ZipFile(self.tmp / "out.zip") as z:
            self.assertEqual(z.read("lib/up/cli"), b"cli")
        write_archive(self.tree, "tar", self.tmp / "out.tar")
        with tarfile.open(self.tmp / "out.tar") as t:
            self.assertEqual(t.getmember("pkg/lib/up").linkname, "../../bin")

    def test_executables(self) -> None:
        # On Windows, only listed files are executable, also in zip's copies of what `bin/cli`
        # and `lib` point to.
        with mock.patch.object(sys, "platform", "win32"):
            write_archive(
                self.tree, "zip", self.tmp / "out.zip", executables=["pkg/lib/x.js"]
            )
        self.assertEqual(
            file_modes((self.tmp / "out.zip").read_bytes(), "zip"),
            {
                "bin/cli": 0o644,
                "lib/x.js": 0o755,
                "pkg/cli.js": 0o644,
                "pkg/lib/x.js": 0o755,
            },
        )
        for listed in ("bin/cli", "lib/x.js"):
            for archive_format in ("zip", "tar"):
                with self.subTest(listed=listed, format=archive_format):
                    out = self.tmp / archive_format
                    error = f"executable {listed} names no file in the directory"
                    with self.assertRaisesRegex(ValueError, re.escape(error)):
                        write_archive(
                            self.tree, archive_format, out, executables=[listed]
                        )
                    self.assertFalse(out.exists())

    def test_rejects(self) -> None:
        absolute = str(self.tree / "pkg" / "cli.js")
        for case, links, error in (
            ("absolute", {"abs": absolute}, f"abs -> {absolute} is absolute"),
            (
                "outside",
                {"d/out": "../../outside.js"},
                "d/out -> ../../outside.js resolves outside",
            ),
            (
                "sibling",
                {"d/s": "../../tree.js"},
                "d/s -> ../../tree.js resolves outside",
            ),
            # `z` points into the directory, but `z/..` leaves it.
            (
                "outside through a symlink",
                {"a": "z/../outside.js", "z": "."},
                "a -> z/../outside.js resolves outside",
            ),
            # These resolve to d/f.js here, but not when extracted to a root of another name.
            (
                "in through the directory's name",
                {"d/in": "../../tree/d/f.js"},
                "d/in -> ../../tree/d/f.js passes outside the directory",
            ),
            (
                "`..` after a symlink",
                {"a": "z/../tree/d/f.js", "z": "."},
                "a -> z/../tree/d/f.js has `..` after a name",
            ),
            (
                "dangling",
                {"d/gone": "missing.js"},
                "d/gone -> missing.js does not resolve",
            ),
            ("loop", {"a": "b", "b": "a"}, "a -> b does not resolve"),
        ):
            for archive_format in ("zip", "tar"):
                with self.subTest(case=case, format=archive_format):
                    parent = Path(tempfile.mkdtemp(dir=self.tmp))
                    (parent / "outside.js").write_bytes(b"outside")
                    (parent / "tree.js").write_bytes(b"sibling")
                    tree = parent / "tree"
                    (tree / "d").mkdir(parents=True)
                    (tree / "d" / "f.js").write_bytes(b"f")
                    for link, target in links.items():
                        os.symlink(target, tree / link)
                    out = parent / "out"
                    with self.assertRaisesRegex(ValueError, re.escape(error)):
                        write_archive(tree, archive_format, out)
                    self.assertFalse(out.exists())

    def test_zip_rejects_symlink_cycles(self) -> None:
        for case, links, error in (
            (
                "parent",
                {"pkg/up": ".."},
                "pkg/up -> .. points to a directory that contains it",
            ),
            ("itself", {"pkg/self": "."}, "pkg/self -> . points to"),
            (
                "mutual",
                {"pkg/lib/to_bin": "../../bin", "bin/to_lib": "../pkg/lib"},
                "bin/to_lib/to_bin -> ../../bin points to",
            ),
        ):
            with self.subTest(case):
                tree = Path(tempfile.mkdtemp(dir=self.tmp)) / "tree"
                shutil.copytree(self.tree, tree, symlinks=True)
                for link, target in links.items():
                    os.symlink(target, tree / link)
                with self.assertRaisesRegex(ValueError, re.escape(error)):
                    write_archive(tree, "zip", self.tmp / "out.zip")
                self.assertFalse((self.tmp / "out.zip").exists())
                write_archive(tree, "tar", self.tmp / "out.tar")
