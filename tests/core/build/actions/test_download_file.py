# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


import asyncio
import hashlib
import socket
from typing import List, Optional

from aiohttp import web
from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase, parametrize
from buck2.tests.e2e_util.helper.http_server import sha1_hex, StaticHttpServer
from buck2.tests.e2e_util.helper.utils import random_string


def configs(
    url: str,
    *,
    sha1: Optional[str] = None,
    sha256: Optional[str] = None,
    size: Optional[int] = None,
) -> List[str]:
    """`-c` arguments declaring the download the fixture's targets read from the config."""
    args = ["-c", f"test.url={url}"]
    if sha1 is not None:
        args += ["-c", f"test.sha1={sha1}"]
    if sha256 is not None:
        args += ["-c", f"test.sha256={sha256}"]
    if size is not None:
        args += ["-c", f"test.size_bytes={size}"]
    return args


def sha256_hex(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def use_fbsource_digests(buck: Buck) -> None:
    """fbsource prefers BLAKE3-KEYED and allows SHA1, so a checksum's algorithm is not the one
    the download would hash with."""
    with open(buck.cwd / ".buckconfig", "a") as buckconfig:
        buckconfig.write("[buck2]\ndigest_algorithms = BLAKE3-KEYED,SHA1\n")


async def build_and_read(buck: Buck, target: str, *args: str) -> bytes:
    result = await buck.build(target, *args)
    return result.get_build_report().output_for_target(target).read_bytes()


class DownloadFileTest(BuckTestCase):
    @buck_test(data_dir="download", skip_for_os=["windows"])
    @parametrize(
        "shape", ["sha1", "sha256", "both", "both_with_size", "sha1_uppercase"]
    )
    async def test_downloads_declared_each_way(self, buck: Buck, shape: str) -> None:
        content = random_string().encode()
        sha1 = sha1_hex(content)
        sha256 = sha256_hex(content)
        declared = {
            "sha1": configs("", sha1=sha1),
            "sha256": configs("", sha256=sha256),
            "both": configs("", sha1=sha1, sha256=sha256),
            "both_with_size": configs("", sha1=sha1, sha256=sha256, size=len(content)),
            "sha1_uppercase": configs("", sha1=sha1.upper()),
        }[shape][2:]
        async with StaticHttpServer({"/file": content}) as server:
            assert (
                await build_and_read(
                    buck,
                    "//:copy",
                    "--local-only",
                    *configs(server.url("/file")),
                    *declared,
                )
                == content
            )
            assert server.count("GET", "/file") == 1

    @buck_test(data_dir="download")
    async def test_retries_transient_errors(self, buck: Buck) -> None:
        routes = web.RouteTableDef()

        attempt = 0
        body: bytes = random_string().encode()

        @routes.get("/")
        async def hello(request: web.Request) -> web.Response:
            nonlocal attempt
            attempt += 1
            if attempt > 2:
                return web.Response(body=body)
            if attempt > 1:
                return web.Response(status=500)
            return web.Response(status=429)

        app = web.Application()
        app.add_routes(routes)

        sock = socket.socket()
        sock.bind(("localhost", 0))

        runner = web.AppRunner(app)
        await runner.setup()
        site = web.SockSite(runner, sock)
        await site.start()

        port = sock.getsockname()[1]
        await buck.build(
            "//:download", *configs(f"http://localhost:{port}", sha1=sha1_hex(body))
        )

        await runner.cleanup()

        # HEAD, then the GET's two retried errors and its success.
        assert attempt == 4

    @buck_test(data_dir="download")
    async def test_a_server_without_head_support(self, buck: Buck) -> None:
        content = random_string().encode()
        async with StaticHttpServer({"/file": content}, allow_head=False) as server:
            assert (
                await build_and_read(
                    buck,
                    "//:download",
                    *configs(server.url("/file"), sha1=sha1_hex(content)),
                )
                == content
            )
            assert server.count("HEAD", "/file") == 1
            assert server.count("GET", "/file") == 1

    @buck_test(data_dir="download")
    async def test_follows_a_relative_location(self, buck: Buck) -> None:
        """A relative `Location` is resolved against the request URI (RFC 3986 section 5.2),
        so the redirect stays on the server instead of going to a host named after the file."""
        content = random_string().encode()
        async with StaticHttpServer(
            {"/dir/artifact.bin": content}, redirects={"/dir/start": "artifact.bin"}
        ) as server:
            assert (
                await build_and_read(
                    buck,
                    "//:download",
                    *configs(server.url("/dir/start"), sha1=sha1_hex(content)),
                )
                == content
            )
            assert server.count("GET", "/dir/start") == 1
            # The HEAD and the GET of `/dir/start` were both redirected to the file.
            assert server.count("GET", "/dir/artifact.bin") >= 1

    @buck_test(data_dir="download")
    async def test_times_out_after_retries(self, buck: Buck) -> None:
        routes = web.RouteTableDef()

        body: bytes = random_string().encode()
        sha1 = sha1_hex(body)

        @routes.get("/always_times_out")
        async def always_times_out(request: web.Request) -> web.Response:
            await asyncio.sleep(3)
            return web.Response(body=body)

        attempt = 0

        @routes.get("/times_out_twice")
        async def times_out_twice(request: web.Request) -> web.Response:
            nonlocal attempt
            attempt += 1
            if attempt > 2:
                return web.Response(body=body)
            await asyncio.sleep(3)
            return web.Response(body=body)

        app = web.Application()
        app.add_routes(routes)

        sock = socket.socket()
        sock.bind(("localhost", 0))

        runner = web.AppRunner(app)
        await runner.setup()
        site = web.SockSite(runner, sock)
        await site.start()

        port = sock.getsockname()[1]
        url = f"http://localhost:{port}"

        # These are daemon startup configs, need these to be written in a buckconfig rather
        # than passed as an invocation config.
        #
        # Add an aggressive read timeout.
        with open(buck.cwd / ".buckconfig", "a") as buckconfig:
            buckconfig.write("[http]\nread_timeout_ms = 50\n")

        await expect_failure(
            buck.build("//:download", *configs(f"{url}/always_times_out", sha1=sha1)),
            stderr_regex="Timed out while making request to",
        )

        await buck.build("//:download", *configs(f"{url}/times_out_twice", sha1=sha1))

        await runner.cleanup()

    @buck_test(data_dir="download")
    async def test_a_missing_url_and_a_dead_server(self, buck: Buck) -> None:
        content = random_string().encode()
        async with StaticHttpServer({"/file": content}) as server:
            await expect_failure(
                buck.build(
                    "//:download", *configs(server.url("/gone"), sha1=sha1_hex(content))
                ),
                stderr_regex="404 Not Found",
            )
            # Nothing listens on port 1.
            await expect_failure(
                buck.build(
                    "//:download",
                    *configs("http://localhost:1/file", sha1=sha1_hex(content)),
                ),
                stderr_regex="Error performing http_download request|Connection refused",
            )

    @buck_test(data_dir="download")
    async def test_declarations_that_cannot_be_right(self, buck: Buck) -> None:
        content = random_string().encode()
        async with StaticHttpServer({"/file": content}) as server:
            url = server.url("/file")
            await expect_failure(
                buck.build("//:download", *configs(url)),
                stderr_regex="Must pass in at least one checksum",
            )
            await expect_failure(
                buck.build("//:download", *configs(url, sha1="xxxxxx")),
                stderr_regex="Invalid digest for `sha1` argument",
            )
            # Nothing was fetched for those.
            assert server.count("GET", "/file") == 0

    @buck_test(data_dir="download")
    async def test_checksums_the_content_does_not_match(self, buck: Buck) -> None:
        content = random_string().encode()
        zeros40 = "0" * 40
        zeros64 = "0" * 64
        async with StaticHttpServer({"/file": content}) as server:
            url = server.url("/file")
            await expect_failure(
                buck.build("//:download", *configs(url, sha1=zeros40)),
                stderr_regex="Invalid sha1 digest",
            )
            await expect_failure(
                buck.build(
                    "//:download", *configs(url, sha1=zeros40, size=len(content))
                ),
                stderr_regex="Invalid sha1 digest",
            )
            await expect_failure(
                buck.build("//:download", *configs(url, sha256=zeros64)),
                stderr_regex="Invalid sha256 digest",
            )
            # Content the CAS does not have is downloaded, and every checksum named is held
            # against it.
            await expect_failure(
                buck.build(
                    "//:download", *configs(url, sha1=sha1_hex(content), sha256=zeros64)
                ),
                stderr_regex="Invalid sha256 digest",
            )

    @buck_test(data_dir="download")
    async def test_a_declared_size_of_zero_is_reported_by_the_download(
        self, buck: Buck
    ) -> None:
        content = random_string().encode()
        async with StaticHttpServer({"/file": content}) as server:
            # A checksum paired with a size of 0 is not the empty file, so the CAS is not asked
            # about it: the download runs and reports the mismatch like any other wrong size.
            await expect_failure(
                buck.build(
                    "//:download",
                    *configs(server.url("/file"), sha1=sha1_hex(content), size=0),
                ),
                stderr_regex=f"Downloaded size \\({len(content)}\\) does not match expected size \\(0\\)",
            )
            assert server.count("GET", "/file") == 1

    @buck_test(data_dir="download")
    async def test_a_wrong_size_is_reported_by_the_download(self, buck: Buck) -> None:
        content = random_string().encode()
        async with StaticHttpServer({"/file": content}) as server:
            await expect_failure(
                buck.build(
                    "//:download",
                    *configs(
                        server.url("/file"),
                        sha1=sha1_hex(content),
                        size=len(content) + 1,
                    ),
                ),
                stderr_regex=f"Downloaded size \\({len(content)}\\) does not match expected size \\({len(content) + 1}\\)",
            )

    @buck_test(data_dir="download", skip_for_os=["windows"])
    async def test_a_content_based_path_under_a_different_preferred_digest(
        self,
        buck: Buck,
    ) -> None:
        # The output's value is what consumers resolve a content-based path from, so it has to be
        # the value the path was built from: the declared one, under the checksum's algorithm.
        use_fbsource_digests(buck)
        content = random_string().encode()
        async with StaticHttpServer({"/file": content}) as server:
            assert (
                await build_and_read(
                    buck,
                    "//:copy_content_based",
                    "--local-only",
                    *configs(server.url("/file"), sha1=sha1_hex(content)),
                )
                == content
            )

    @buck_test(data_dir="download")
    @parametrize("digest_algorithm", ["SHA1", "SHA256"])
    async def test_fetches_once_across_restarts(
        self, buck: Buck, digest_algorithm: str
    ) -> None:
        with open(buck.cwd / ".buckconfig", "a") as f:
            f.write("[buck2]\n")
            f.write(f"digest_algorithms = {digest_algorithm}\n")
            f.write("sqlite_materializer_state = true\n")

        content = random_string().encode()
        async with StaticHttpServer({"/file": content}) as server:
            declared = configs(
                server.url("/file"), sha1=sha1_hex(content), sha256=sha256_hex(content)
            )
            target = "//:download"

            res = await buck.build(target, *declared)
            output = res.get_build_report().output_for_target(target)
            assert output.read_bytes() == content
            assert server.count("GET", "/file") == 1

            # The file is still on disk, so a new daemon has no reason to fetch it again.
            await buck.kill()
            await buck.build(target, *declared)
            assert output.read_bytes() == content
            assert server.count("GET", "/file") == 1

    @buck_test(data_dir="download")
    async def test_is_uploaded_from_disk(self, buck: Buck) -> None:
        content = random_string().encode()
        # Force the uploader to treat the download as missing from the CAS even if it is there.
        buck.set_env(
            "BUCK2_TEST_INJECTED_MISSING_DIGESTS",
            f"{sha1_hex(content)}:{len(content)}",
        )
        async with StaticHttpServer({"/file": content}) as server:
            await buck.build(
                "//:copy",
                "--remote-only",
                "--no-remote-cache",
                *configs(server.url("/file"), sha1=sha1_hex(content)),
            )
            assert server.count("GET", "/file") == 1

    @buck_test(data_dir="download")
    async def test_uses_the_cas_when_it_has_the_content(self, buck: Buck) -> None:
        content = random_string().encode()
        async with StaticHttpServer({"/file": content}) as server:
            declared = configs(server.url("/file"), sha1=sha1_hex(content))

            # Running an RE action on the download uploads its content to the CAS.
            await buck.build("//:copy", "--remote-only", "--no-remote-cache", *declared)
            assert server.count("GET", "/file") == 1

            # A second download of the same content finds it in the CAS, so it neither contacts
            # the server nor touches the disk until something needs the file...
            target = "//:download_again"
            res = await buck.build(target, "--materializations=none", *declared)
            output = res.get_build_report().output_for_target(target)
            assert not output.exists()
            assert server.count("GET", "/file") == 1

            # ...and then it comes out of the CAS.
            await buck.build(target, *declared)
            assert output.read_bytes() == content
            assert server.count("GET", "/file") == 1

    @buck_test(data_dir="download")
    async def test_content_the_cas_holds_needs_no_url(self, buck: Buck) -> None:
        content = random_string().encode()
        async with StaticHttpServer({"/file": content}) as server:
            # With a size declared, the download is fully described without contacting the URL.
            declared = configs(
                server.url("/file"), sha1=sha1_hex(content), size=len(content)
            )
            await buck.build("//:copy", "--remote-only", "--no-remote-cache", *declared)
            assert server.count("GET", "/file") == 1

            # A new daemon and a URL nothing answers on: the declaration names content the CAS
            # holds, so a remote consumer and a local one both get it without the URL.
            await buck.kill()
            broken = configs(
                "http://localhost:1/file", sha1=sha1_hex(content), size=len(content)
            )
            await buck.build("//:copy", "--remote-only", "--no-remote-cache", *broken)
            assert (
                await build_and_read(buck, "//:copy_again", "--local-only", *broken)
                == content
            )
            assert server.count("GET", "/file") == 1

    @buck_test(data_dir="download")
    async def test_a_second_checksum_is_not_checked_against_content_the_cas_serves(
        self,
        buck: Buck,
    ) -> None:
        content = random_string().encode()
        async with StaticHttpServer({"/file": content}) as server:
            await buck.build(
                "//:copy",
                "--remote-only",
                "--no-remote-cache",
                *configs(server.url("/file"), sha1=sha1_hex(content)),
            )
            assert server.count("GET", "/file") == 1

            # The CAS vouches for the sha1 and serves the content under it; the sha256 the
            # declaration also names is never held against bytes nobody fetches.
            assert (
                await build_and_read(
                    buck,
                    "//:copy_again",
                    "--local-only",
                    *configs(
                        server.url("/file"), sha1=sha1_hex(content), sha256="0" * 64
                    ),
                )
                == content
            )
            assert server.count("GET", "/file") == 1

    @buck_test(data_dir="download")
    async def test_a_wrong_size_is_a_miss_in_the_cas(self, buck: Buck) -> None:
        content = random_string().encode()
        async with StaticHttpServer({"/file": content}) as server:
            await buck.build(
                "//:copy",
                "--remote-only",
                "--no-remote-cache",
                *configs(server.url("/file"), sha1=sha1_hex(content)),
            )
            assert server.count("GET", "/file") == 1

            # The CAS names content by hash and size, so the content is not there under the wrong
            # size: the download runs and reports the mismatch.
            await expect_failure(
                buck.build(
                    "//:download_again",
                    *configs(
                        server.url("/file"),
                        sha1=sha1_hex(content),
                        size=len(content) + 1,
                    ),
                ),
                stderr_regex=f"Downloaded size \\({len(content)}\\) does not match expected size \\({len(content) + 1}\\)",
            )
            assert server.count("GET", "/file") == 2
