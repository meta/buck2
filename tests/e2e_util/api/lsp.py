# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import contextlib
import copy
import json
import re
from asyncio import subprocess, wait_for
from asyncio.streams import StreamReader, StreamWriter
from collections import deque
from pathlib import Path
from typing import Any, Optional, Self

CONTENT_HEADER_REGEX = re.compile(r"Content-Length: (\d+)")


class LSPResponseError(Exception):
    def __init__(self, json_error: Any) -> None:
        super().__init__(f"Error returned from LSP: `{json_error}`")
        self.json_error = json_error


class LspClient(contextlib.AbstractAsyncContextManager):
    def __init__(self, process: subprocess.Process, cwd: Path) -> None:
        self.process = process
        self.cwd = cwd
        self.id = 0
        self.notifications = deque()
        self.responses = {}

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(self, exc_type, exc, tb) -> None:
        try:
            self.process.kill()
        except ProcessLookupError:
            pass
        # Yes, we really wait that long. Process startup time is slow
        # sometimes in dev builds, the box can be overloaded, etc.
        # Average case is much lower.
        await wait_for(self.process.wait(), timeout=120)

    def stdin(self) -> StreamWriter:
        if self.process.stdin is None:
            raise ValueError("Tried to fetch stdin, but it was not set")
        else:
            return self.process.stdin

    def stdout(self) -> StreamReader:
        if self.process.stdout is None:
            raise ValueError("Tried to fetch stdout, but it was not set")
        else:
            return self.process.stdout

    async def write_message(self, payload: Any) -> None:
        payload_bytes = json.dumps(payload).encode("utf-8")
        self.stdin().write(
            f"Content-Length: {len(payload_bytes)}\r\n\r\n".encode("utf-8")
        )
        self.stdin().write(payload_bytes)
        await self.stdin().drain()

    async def _read_message(self) -> Any:
        content_length = (await self.stdout().readline()).decode("utf-8")
        await self.stdout().readline()
        matches = CONTENT_HEADER_REGEX.match(content_length)
        if not matches:
            raise ValueError(
                f"Did not get content length header. Got `{content_length}`"
            )
        content_length_bytes = int(matches.group(1))
        content = (await self.stdout().read(content_length_bytes)).decode("utf-8")

        js = json.loads(content)
        if "id" in js:
            self.responses[js["id"]] = js
        else:
            self.notifications.append(js)
        return js

    async def read_message(self) -> Any:
        return await wait_for(self._read_message(), timeout=30)

    async def open_file(
        self, relative_path: Path, contents: Optional[str] = None
    ) -> Optional[Any]:
        absolute_path = self.cwd / relative_path
        if contents is None:
            with open(absolute_path, encoding="utf-8") as fin:
                contents = fin.read()

        payload = {
            "textDocument": {
                "uri": absolute_path.as_uri(),
                "languageId": "starlark",
                "version": 1,
                "text": contents,
            }
        }
        await self.send_notification("textDocument/didOpen", payload)
        notif = await self.receive_notification("textDocument/publishDiagnostics")

        assert notif is not None
        assert notif["params"]["uri"] == absolute_path.as_uri()
        return notif["params"]

    async def goto_definition(
        self, relative_path: Path, line: int, col: int
    ) -> Optional[Any]:
        absolute_path = self.cwd / relative_path
        payload = {
            "textDocument": {
                "uri": absolute_path.as_uri(),
            },
            "position": {
                "line": line,
                "character": col,
            },
        }

        req_id = await self.send_request("textDocument/definition", payload)
        return await self.receive_response(req_id)

    async def file_contents(self, uri: str) -> Optional[Any]:
        payload = {
            "uri": uri,
        }

        req_id = await self.send_request("starlark/fileContents", payload)
        return await self.receive_response(req_id)

    async def send_notification(self, method: str, notification: Any) -> None:
        payload = {"jsonrpc": "2.0", "method": method, "params": notification}
        await self.write_message(payload)

    async def receive_notification(
        self, method: Optional[str] = None, retries: int = 10
    ) -> Optional[Any]:
        for _ in range(0, retries):
            await self.read_message()
            try:
                (idx, value) = next(
                    (
                        (i, v)
                        for (i, v) in enumerate(self.notifications)
                        if (method is None or v["method"] == method)
                    ),
                    (-1, None),
                )
                if value is not None:
                    del self.notifications[idx]
                    return value

            except (IndexError, ValueError):
                pass

        return None

    async def send_request(self, method: str, request: Any) -> int:
        self.id += 1
        id = self.id
        payload = {"jsonrpc": "2.0", "id": id, "method": method, "params": request}
        await self.write_message(payload)
        return id

    async def receive_response(
        self, request_id: int, retries: int = 10
    ) -> Optional[Any]:
        for _ in range(0, retries):
            await self.read_message()
            if request_id not in self.responses:
                continue
            response = self.responses.pop(request_id)
            if response.get("result") is not None:
                return response["result"]
            else:
                raise LSPResponseError(response["error"])
        return None

    async def init_connection(self) -> None:
        repo_uri = self.cwd.as_uri()
        request = copy.deepcopy(_INIT_REQUEST)
        request["workspaceFolders"][0]["uri"] = repo_uri
        request["rootUri"] = repo_uri

        req_id = await self.send_request("initialize", request)
        response = await self.receive_response(req_id)

        # pyrefly: ignore [unsupported-operation]
        assert "definitionProvider" in response["capabilities"]
        assert "textDocumentSync" in response["capabilities"]
        await self.send_notification("initialized", {})


# Sample initialization request sent from vscode.
_INIT_REQUEST = {
    "capabilities": {
        "textDocument": {
            "completion": {
                "completionItem": {
                    "commitCharactersSupport": True,
                    "deprecatedSupport": True,
                    "documentationFormat": ["markdown", "plaintext"],
                    "preselectSupport": True,
                    "snippetSupport": True,
                    "tagSupport": {"valueSet": [1]},
                },
                "completionItemKind": {
                    "valueSet": [
                        1,
                        2,
                        3,
                        4,
                        5,
                        6,
                        7,
                        8,
                        9,
                        10,
                        11,
                        12,
                        13,
                        14,
                        15,
                        16,
                        17,
                        18,
                        19,
                        20,
                        21,
                        22,
                        23,
                        24,
                        25,
                    ]
                },
                "contextSupport": True,
                "dynamicRegistration": True,
            },
            "declaration": {"dynamicRegistration": True, "linkSupport": True},
            "definition": {"dynamicRegistration": True, "linkSupport": True},
            "documentHighlight": {"dynamicRegistration": True},
            "documentLink": {"dynamicRegistration": True, "tooltipSupport": True},
            "documentSymbol": {
                "dynamicRegistration": True,
                "hierarchicalDocumentSymbolSupport": True,
                "symbolKind": {
                    "valueSet": [
                        1,
                        2,
                        3,
                        4,
                        5,
                        6,
                        7,
                        8,
                        9,
                        10,
                        11,
                        12,
                        13,
                        14,
                        15,
                        16,
                        17,
                        18,
                        19,
                        20,
                        21,
                        22,
                        23,
                        24,
                        25,
                        26,
                    ]
                },
            },
            "foldingRange": {
                "dynamicRegistration": True,
                "lineFoldingOnly": True,
                "rangeLimit": 5000,
            },
            "formatting": {"dynamicRegistration": True},
            "hover": {
                "contentFormat": ["markdown", "plaintext"],
                "dynamicRegistration": True,
            },
            "implementation": {"dynamicRegistration": True, "linkSupport": True},
            "onTypeFormatting": {"dynamicRegistration": True},
            "publishDiagnostics": {
                "relatedInformation": True,
                "tagSupport": {"valueSet": [1, 2]},
                "versionSupport": False,
            },
            "rangeFormatting": {"dynamicRegistration": True},
            "references": {"dynamicRegistration": True},
            "rename": {"dynamicRegistration": True, "prepareSupport": True},
            "selectionRange": {"dynamicRegistration": True},
            "signatureHelp": {
                "contextSupport": True,
                "dynamicRegistration": True,
                "signatureInformation": {
                    "documentationFormat": ["markdown", "plaintext"],
                    "parameterInformation": {"labelOffsetSupport": True},
                },
            },
            "synchronization": {
                "didSave": True,
                "dynamicRegistration": True,
                "willSave": True,
                "willSaveWaitUntil": True,
            },
            "typeDefinition": {"dynamicRegistration": True, "linkSupport": True},
        },
        "window": {"workDoneProgress": True},
    },
    "processId": None,
    "rootUri": "file:///INVALID",
    "trace": "off",
    "workspaceFolders": [{"name": "buck2", "uri": "file:///INVALID"}],
}
