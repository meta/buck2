# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import hashlib
import socket
from typing import Dict, List, Optional, Tuple, Union

from aiohttp import web


class StaticHttpServer:
    """
    Serves fixed content on localhost and records every request, so a test can assert on what a
    download actually fetched. `files` maps what a client asks for to the content to answer with,
    or is the content to answer everything with, which is what a server standing in for a proxy
    usually wants. A key is matched against the request target first and the path second, so a
    proxy can be given one entry per absolute URL it will be asked to fetch.

    Recorded requests are `(method, target)`, where the target is what the client asked for: a
    path for a direct request, an absolute URL for one addressed to a proxy. `redirects` maps
    those same targets to a `Location` to answer with instead. With `allow_head=False` the server
    answers HEAD requests with 405, like servers that do not implement HEAD.
    """

    def __init__(
        self,
        files: Union[Dict[str, bytes], bytes],
        *,
        allow_head: bool = True,
        host: str = "localhost",
        redirects: Optional[Dict[str, str]] = None,
    ) -> None:
        self._files = files
        self._allow_head = allow_head
        self._host = host
        self._redirects = redirects or {}
        self.requests: List[Tuple[str, str]] = []
        self._runner: Optional[web.AppRunner] = None
        self._port: Optional[int] = None

    async def __aenter__(self) -> "StaticHttpServer":
        app = web.Application()
        app.router.add_route("*", "/{path:.*}", self._handle)
        sock = socket.socket()
        sock.bind((self._host, 0))
        self._port = sock.getsockname()[1]
        self._runner = web.AppRunner(app, access_log=None)
        await self._runner.setup()
        await web.SockSite(self._runner, sock).start()
        return self

    async def __aexit__(self, *exc: object) -> None:
        assert self._runner is not None
        await self._runner.cleanup()

    def url(self, path: str = "") -> str:
        return f"http://{self._host}:{self._port}{path}"

    def count(self, method: str, path: str) -> int:
        return sum(1 for m, p in self.requests if m == method and p == path)

    async def _handle(self, request: web.Request) -> web.Response:
        target = request.raw_path
        self.requests.append((request.method, target))
        if target in self._redirects:
            return web.Response(
                status=302, headers={"Location": self._redirects[target]}
            )
        if isinstance(self._files, bytes):
            content = self._files
        else:
            content = self._files.get(target, self._files.get(request.path))
            if content is None:
                return web.Response(status=404)
        if request.method == "HEAD" and not self._allow_head:
            return web.Response(status=405)
        if request.method not in ("GET", "HEAD"):
            return web.Response(status=405)
        # aiohttp drops the body of a HEAD response itself and keeps `Content-Length`.
        return web.Response(body=content)


def sha1_hex(content: bytes) -> str:
    """The checksum `download_file` takes is a sha1: a protocol, not a security choice."""
    h = hashlib.sha1(content)  # patternlint-disable-line poor-choice-of-hash-function
    return h.hexdigest()


def download_configs(url: str, content: bytes) -> List[str]:
    """
    `-c` arguments for fixtures whose `download_file` rules read `test.url`, `test.sha1` and
    `test.sha256` from the config.
    """
    return [
        "-c",
        f"test.url={url}",
        "-c",
        f"test.sha1={sha1_hex(content)}",
        "-c",
        f"test.sha256={hashlib.sha256(content).hexdigest()}",
    ]
