# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


import asyncio
import io
import json
import os
import shutil
import signal
import tarfile
import zipfile
from pathlib import Path

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import (
    buck_test,
    BuckTestCase,
    parametrize,
)
from buck2.tests.e2e_util.helper.utils import filter_events, get_last_execution_kind

FILES: dict[str, tuple[bytes, int]] = {
    "pkg-1.0/README": (b"hello\n", 0o644),
    "pkg-1.0/bin/tool": (b"#!/bin/sh\n", 0o755),
    "pkg-1.0/data_test.txt": (b"excluded\n", 0o644),
}


def _tar(mode: str) -> bytes:
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode=mode) as tar:
        for name in ["pkg-1.0", "pkg-1.0/bin"]:
            info = tarfile.TarInfo(name)
            info.type = tarfile.DIRTYPE
            info.mode = 0o755
            tar.addfile(info)
        for name, (contents, perms) in FILES.items():
            info = tarfile.TarInfo(name)
            info.size = len(contents)
            info.mode = perms
            tar.addfile(info, io.BytesIO(contents))
        info = tarfile.TarInfo("pkg-1.0/README.md")
        info.type = tarfile.SYMTYPE
        info.linkname = "README"
        tar.addfile(info)
    return buf.getvalue()


def _zip() -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        for name, (contents, perms) in FILES.items():
            info = zipfile.ZipInfo(name)
            info.external_attr = (0o100000 | perms) << 16
            z.writestr(info, contents)
    return buf.getvalue()


def _sparse_tar(size: int, names: list[str]) -> bytes:
    first = tarfile.TarInfo("pkg-1.0/README")
    first.size = 4
    payload = b"data" + b"\0" * 508
    data = bytearray(first.tobuf(format=tarfile.GNU_FORMAT) + payload)
    for name in names:
        sparse = tarfile.TarInfo(name)
        sparse.type = tarfile.GNUTYPE_SPARSE
        sparse.size = 4
        header = bytearray(sparse.tobuf(format=tarfile.GNU_FORMAT))
        header[386:398] = (size - 4).to_bytes(12, "big")
        header[386] |= 0x80
        header[398:410] = b"00000000004\0"
        header[483:495] = size.to_bytes(12, "big")
        header[483] |= 0x80
        header[148:156] = b" " * 8
        header[148:156] = f"{sum(header):06o}\0 ".encode()
        data.extend(header + payload)
    return bytes(data) + b"\0" * 1024


def _open_sparse_readers(pid: int) -> list[str]:
    readers = []
    for fd in Path(f"/proc/{pid}/fd").iterdir():
        try:
            name = os.readlink(fd)
            info = dict(
                line.split(":", 1)
                for line in Path(f"/proc/{pid}/fdinfo/{fd.name}").read_text().splitlines()
            )
        except FileNotFoundError:
            continue
        if name.removesuffix(" (deleted)").endswith("/slow") and (
            int(info["flags"], 8) & os.O_ACCMODE == os.O_RDONLY
        ):
            readers.append(name)
    return readers


def _write_archives(root: Path) -> None:
    (root / "archive.tar.gz").write_bytes(_tar("w:gz"))
    (root / "archive.bin").write_bytes(_tar("w:gz"))
    (root / "archive.tar.xz").write_bytes(_tar("w:xz"))
    (root / "archive.zip").write_bytes(_zip())

    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tar:
        info = tarfile.TarInfo("a/../../escape")
        info.size = 1
        tar.addfile(info, io.BytesIO(b"x"))
    (root / "parent_dir.tar").write_bytes(buf.getvalue())


def _listing(root: Path) -> list[str]:
    out = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        for name in sorted(dirnames + filenames):
            path = Path(dirpath) / name
            rel = path.relative_to(root).as_posix()
            if path.is_symlink():
                out.append(f"{rel} -> {os.readlink(path)}")
            elif path.is_dir():
                out.append(f"{rel}/")
            else:
                x = " (x)" if os.access(path, os.X_OK) else ""
                out.append(f"{rel}: {path.read_text().strip()}{x}")
    return sorted(out)


async def _build_listing(buck: Buck, target: str) -> list[str]:
    result = await buck.build(target)
    return _listing(result.get_build_report().output_for_target(target))


def _enable_persistence(buck: Buck) -> None:
    with (buck.cwd / ".buckconfig").open("a") as config:
        config.write(
            "\n[buck2]\nsqlite_materializer_state = true\nsqlite_dep_file_state = true\n"
        )


async def _build_cached(buck: Buck, *options: str) -> Path:
    result = await buck.build("root//:cached", "--no-buckd", *options)
    return result.get_build_report().output_for_target("root//:cached").resolve()


class ExtractArchiveTest(BuckTestCase):
    @buck_test(skip_for_os=["windows"])
    async def test_extract_tar(self, buck: Buck) -> None:
        # Tar decoders must apply prefix and exclusion rules while preserving file contents,
        # symlinks and executable bits.
        _write_archives(buck.cwd)
        expected = [
            "README.md -> README",
            "README: hello",
            "bin/",
            "bin/tool: #!/bin/sh (x)",
        ]
        assert await _build_listing(buck, "root//:tar_gz") == expected
        expected.append("data_test.txt: excluded")
        assert await _build_listing(buck, "root//:tar_xz") == expected
        assert await _build_listing(buck, "root//:explicit_type") == expected

    @buck_test(skip_for_os=["windows"])
    async def test_extract_zip(self, buck: Buck) -> None:
        # ZIP extraction must remove the requested prefix and preserve the selected files and
        # executable bits.
        _write_archives(buck.cwd)
        assert await _build_listing(buck, "root//:zip") == [
            "README: hello",
            "bin/",
            "bin/tool: #!/bin/sh (x)",
            "data_test.txt: excluded",
        ]

    @buck_test(skip_for_os=["windows"])
    async def test_extract_content_based(self, buck: Buck) -> None:
        # A consumer must read a published extraction, and successful publication must remove its
        # scratch tree.
        _write_archives(buck.cwd)
        result = await buck.build("root//:cat_content_based")
        out = result.get_build_report().output_for_target("root//:cat_content_based")
        assert out.read_text() == "hello\n"
        assert not list((buck.cwd / "buck-out").glob("*/tmp/**/content_based"))

    @buck_test(skip_for_os=["windows"])
    async def test_extract_reruns_when_archive_changes(self, buck: Buck) -> None:
        # Changing an archive in one daemon must rebuild the extraction with the replacement file
        # contents.
        _write_archives(buck.cwd)
        assert "README: hello" in await _build_listing(buck, "root//:tar_gz")

        buf = io.BytesIO()
        with tarfile.open(fileobj=buf, mode="w:gz") as tar:
            info = tarfile.TarInfo("pkg-1.0/README")
            info.size = 4
            tar.addfile(info, io.BytesIO(b"bye\n"))
        (buck.cwd / "archive.tar.gz").write_bytes(buf.getvalue())

        assert await _build_listing(buck, "root//:tar_gz") == ["README: bye"]

    @buck_test(skip_for_os=["windows"])
    async def test_extract_errors(self, buck: Buck) -> None:
        # Invalid member paths and unsupported archive types must produce input errors.
        _write_archives(buck.cwd)
        await expect_failure(
            buck.build("root//:parent_dir"),
            stderr_regex="Archive entry `a/../../escape` has an invalid name",
        )
        await expect_failure(
            buck.build("root//:bad_type"),
            stderr_regex="Unknown archive type `rar`",
        )


    @buck_test(skip_for_os=["windows"])
    @parametrize("content_based", [False, True])
    async def test_reuse_across_daemonless_invocations(
        self, buck: Buck, content_based: bool
    ) -> None:
        # Two daemonless builds with persistence enabled must reuse the extraction and retain
        # output inodes and timestamps.
        _write_archives(buck.cwd)
        _enable_persistence(buck)
        options = ["-c", f"extract.content_based={str(content_based).lower()}"]
        # The first invocation must extract the archive.
        output = await _build_cached(buck, *options)
        assert await get_last_execution_kind(buck, "extract_archive") == 4
        before = (output / "README").stat()
        # A separate invocation must recover the persisted output without writing it again.
        assert await _build_cached(buck, *options) == output
        assert await get_last_execution_kind(buck, "extract_archive") == 10
        after = (output / "README").stat()
        assert (before.st_ino, before.st_mtime_ns) == (after.st_ino, after.st_mtime_ns)
        assert "README: hello" in _listing(output)

    @buck_test(skip_for_os=["windows"])
    @parametrize("content_based", [False, True])
    @parametrize("change", ["archive", "strip_prefix", "excludes", "missing", "invalidated"])
    async def test_daemonless_cache_misses(
        self, buck: Buck, content_based: bool, change: str
    ) -> None:
        # Changing archive content or options, deleting output, or invalidating persisted state
        # must extract again before reuse.
        _write_archives(buck.cwd)
        _enable_persistence(buck)
        options = ["-c", f"extract.content_based={str(content_based).lower()}"]
        output = await _build_cached(buck, *options)
        # Change the extraction identity or invalidate the previous output.
        if change == "archive":
            buf = io.BytesIO()
            with tarfile.open(fileobj=buf, mode="w:gz") as tar:
                info = tarfile.TarInfo("pkg-1.0/README")
                info.size = 8
                tar.addfile(info, io.BytesIO(b"changed\n"))
            (buck.cwd / "archive.bin").write_bytes(buf.getvalue())
        elif change == "strip_prefix":
            options += ["-c", "extract.strip_prefix=pkg-1.0/bin"]
        elif change == "excludes":
            options += ["-c", "extract.excludes=README"]
        elif change == "missing":
            shutil.rmtree(output)
        else:
            with (buck.cwd / ".buckconfig").open("a") as config:
                config.write("\n[buck2]\nsqlite_materializer_state_version = 100\n")
        # The next invocation must extract again and make the replacement reusable.
        output = await _build_cached(buck, *options)
        assert await get_last_execution_kind(buck, "extract_archive") == 4
        listing = _listing(output)
        if change == "strip_prefix":
            assert listing == ["tool: #!/bin/sh (x)"]
        elif change == "excludes":
            assert not any("README" in entry for entry in listing)
        elif change == "archive":
            assert listing == ["README: changed"]
        else:
            assert "README: hello" in listing
        await _build_cached(buck, *options)
        assert await get_last_execution_kind(buck, "extract_archive") == 10

    @buck_test(skip_for_os=["windows"])
    async def test_failed_extraction_is_not_reused(self, buck: Buck) -> None:
        # A failed extraction after a successful build must invalidate its output and require
        # reconstruction when the archive is restored.
        _write_archives(buck.cwd)
        _enable_persistence(buck)
        await _build_cached(buck)
        archive = buck.cwd / "archive.bin"
        good = archive.read_bytes()
        # A bad archive must fail after invalidating the previous extraction.
        archive.write_bytes((buck.cwd / "parent_dir.tar").read_bytes())
        await expect_failure(
            _build_cached(buck, "-c", "extract.type=tar"),
            stderr_regex="invalid name",
        )
        # Restoring the archive must rebuild the invalidated output before it can be reused.
        archive.write_bytes(good)
        output = await _build_cached(buck)
        assert await get_last_execution_kind(buck, "extract_archive") == 4
        assert "README: hello" in _listing(output)
        await _build_cached(buck)
        assert await get_last_execution_kind(buck, "extract_archive") == 10

    @buck_test(skip_for_os=["windows"])
    @parametrize("failure", ["entry", "checksum", "truncated"])
    async def test_failed_content_based_extraction_cleans_scratch(
        self, buck: Buck, failure: str
    ) -> None:
        # An archive that fails after writing files must leave no extraction scratch output.
        # Repeating the failure must fail again, and a repaired archive must extract before reuse.
        _write_archives(buck.cwd)
        _enable_persistence(buck)
        archive = buck.cwd / "archive.bin"
        good = archive.read_bytes()
        options = ["-c", "extract.content_based=true"]
        if failure == "entry":
            buf = io.BytesIO()
            with tarfile.open(fileobj=buf, mode="w") as tar:
                info = tarfile.TarInfo("pkg-1.0/README")
                info.size = 4
                tar.addfile(info, io.BytesIO(b"data"))
                info = tarfile.TarInfo("pkg-1.0/../../escape")
                tar.addfile(info)
            archive.write_bytes(buf.getvalue())
            options += ["-c", "extract.type=tar"]
        elif failure == "checksum":
            damaged = bytearray(good)
            damaged[-8] ^= 0x80
            archive.write_bytes(damaged)
        else:
            archive.write_bytes(good[:-8])

        # Failure must neither retain the partial tree nor make that archive reusable.
        for _ in range(2):
            await expect_failure(_build_cached(buck, *options))
            assert not list((buck.cwd / "buck-out").glob("*/tmp/**/README"))

        # Successful extraction after repair must still populate the persistent cache.
        archive.write_bytes(good)
        options = ["-c", "extract.content_based=true"]
        output = await _build_cached(buck, *options)
        assert "README: hello" in _listing(output)
        assert await get_last_execution_kind(buck, "extract_archive") == 4
        await _build_cached(buck, *options)
        assert await get_last_execution_kind(buck, "extract_archive") == 10

    @buck_test(skip_for_os=["windows"])
    async def test_cancelled_extraction_cleans_scratch(self, buck: Buck) -> None:
        # Cancelling a daemonless extraction after it writes a file must remove its scratch tree.
        # A sparse member keeps extraction active while storing only four data bytes in the archive.
        _write_archives(buck.cwd)
        _enable_persistence(buck)
        archive = buck.cwd / "archive.bin"
        good = archive.read_bytes()
        archive.write_bytes(_sparse_tar(1 << 40, ["pkg-1.0/slow"]))
        process = await buck.build(
            "root//:cached",
            "--no-buckd",
            "-c", "extract.content_based=true",
            "-c", "extract.type=tar",
        ).start()
        try:
            async with asyncio.timeout(30):
                while not list((buck.cwd / "buck-out").glob("*/tmp/**/README")):
                    if process.returncode is not None:
                        _, stderr = await process.communicate()
                        self.fail(stderr.decode())
                    await asyncio.sleep(0.01)
                process.send_signal(signal.SIGINT)
                _, stderr = await process.communicate()
            assert process.returncode != 0, stderr.decode()
            assert not list((buck.cwd / "buck-out").glob("*/tmp/**/README"))
        finally:
            if process.returncode is None:
                process.kill()
                await process.wait()
        # A repaired archive must still execute successfully and become reusable.
        archive.write_bytes(good)
        await _build_cached(buck, "-c", "extract.content_based=true")
        assert await get_last_execution_kind(buck, "extract_archive") == 4
        await _build_cached(buck, "-c", "extract.content_based=true")
        assert await get_last_execution_kind(buck, "extract_archive") == 10

    @buck_test(skip_for_os=["windows", "darwin"])
    async def test_cancelled_hashing_stops_readers_before_cleanup(self, buck: Buck) -> None:
        # Cancelling output hashing must close every sparse-file reader before removing scratch.
        # Two large sparse members in separate directories exercise parallel recursive hashing.
        _write_archives(buck.cwd)
        _enable_persistence(buck)
        archive = buck.cwd / "archive.bin"
        good = archive.read_bytes()
        archive.write_bytes(
            _sparse_tar(1 << 34, ["pkg-1.0/one/slow", "pkg-1.0/two/slow"])
        )
        await buck.targets("root//:cached")
        pid = json.loads((await buck.status()).stdout)["process_info"]["pid"]
        process = await buck.build(
            "root//:cached",
            "-c", "extract.content_based=true",
            "-c", "extract.type=tar",
        ).start()
        try:
            async with asyncio.timeout(45):
                # Read-only descriptors distinguish hashing from extraction's output writers.
                while len(_open_sparse_readers(pid)) != 2:
                    if process.returncode is not None:
                        _, stderr = await process.communicate()
                        self.fail(stderr.decode())
                    await asyncio.sleep(0.005)
                process.send_signal(signal.SIGINT)
                _, stderr = await process.communicate()
                # Scratch removal proves that the cancelled action has released its output.
                while list((buck.cwd / "buck-out").glob("*/tmp/**/slow")):
                    await asyncio.sleep(0.005)
            assert process.returncode != 0, stderr.decode()
            assert not _open_sparse_readers(pid)
        finally:
            if process.returncode is None:
                process.kill()
                await process.wait()
        # A repaired archive must execute again before becoming reusable after a daemon restart.
        await buck.kill()
        archive.write_bytes(good)
        await _build_cached(buck, "-c", "extract.content_based=true")
        assert await get_last_execution_kind(buck, "extract_archive") == 4
        await _build_cached(buck, "-c", "extract.content_based=true")
        assert await get_last_execution_kind(buck, "extract_archive") == 10

    @buck_test(skip_for_os=["windows"])
    async def test_archive_type_is_part_of_cache_identity(self, buck: Buck) -> None:
        # Changing the declared archive type must reject the old cached extraction and execute
        # the incompatible decoder.
        _write_archives(buck.cwd)
        _enable_persistence(buck)
        await _build_cached(buck)
        await expect_failure(_build_cached(buck, "-c", "extract.type=zip"))
        await _build_cached(buck)
        assert await get_last_execution_kind(buck, "extract_archive") == 4


    @buck_test(skip_for_os=["windows"])
    async def test_reuse_after_concurrent_content_based_extractions(
        self, buck: Buck
    ) -> None:
        # Concurrent configured consumers must read their published extractions, then reuse them
        # across a daemonless restart.
        _write_archives(buck.cwd)
        _enable_persistence(buck)
        targets = [f"root//:consumer_{i}" for i in range(4)]
        # Strict materializers discard intermediates, so retain the archives as final outputs.
        archives = [f"{target}[archive]" for target in targets]
        for expected_kind in [4, 10]:
            result = await buck.build(*targets, *archives, "--no-buckd")
            for target in targets:
                output = result.get_build_report().output_for_target(target)
                assert output.read_text() == "hello\n"
            executions = await filter_events(
                buck, "Event", "data", "SpanEnd", "data", "ActionExecution"
            )
            kinds = [
                action["execution_kind"]
                for action in executions
                if action["name"]["category"] == "extract_archive"
            ]
            assert kinds == [expected_kind] * 4
