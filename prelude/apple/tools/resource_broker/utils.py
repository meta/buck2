# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import asyncio
import json
import shlex

from .timeouts import DEFAULT_OPERATION_TIMEOUT


async def _read_until_valid_json(stream: asyncio.StreamReader) -> object:
    buffer = b""
    while True:
        data = await stream.readuntil(b"}")
        buffer += data
        try:
            return json.loads(buffer.decode())
        except json.JSONDecodeError:
            pass
    raise RuntimeError(
        "Should not be reachable since either the valid JSON is there or `asyncio.IncompleteReadError` is raised."
    )


async def execute_generic_text_producing_command(
    name: str, cmd: list[str], timeout: float = DEFAULT_OPERATION_TIMEOUT
) -> str:
    process = await asyncio.create_subprocess_exec(
        *cmd,
        stdin=asyncio.subprocess.DEVNULL,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    stdout, stderr = await asyncio.wait_for(process.communicate(), timeout=timeout)
    if process.returncode != 0:
        raise RuntimeError(
            f"Failed to {name} with command:\n```\n{shlex.join(cmd)}\n```\nstdout:\n```\n{stdout.decode(errors='ignore')}\n```\nstderr:\n```\n{stderr.decode(errors='ignore')}\n```\n"
        )
    return stdout.decode()
