#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import signal
from asyncio import subprocess
from pathlib import Path
from typing import (
    Any,
    Awaitable,
    Dict,
    Generator,
    Generic,
    IO,
    Iterable,
    Optional,
    Union,
)

from buck2.tests.e2e_util.api.result import E, ExceptionType, R, ResultType


class Process(Generic[R, E], Awaitable[R]):
    """Instantiates a BuckProcess object with a running process"""

    def __init__(
        self,
        *,
        cmd_to_run: Iterable[str],
        working_dir: Path,
        env: Dict[str, str],
        input: Union[bytes, None],
        stdin: Optional[int],
        stdout: Union[int, IO[Any], None],
        stderr: Union[int, IO[Any], None],
        result_type: ResultType,
        exception_type: ExceptionType,
        encoding: str,
    ) -> None:
        self.cmd_to_run = cmd_to_run
        self.working_dir = working_dir
        self.env = env
        self.input = input
        self._result_type = result_type
        self._exception_type = exception_type
        self._encoding = encoding
        if stdin is None:
            stdin = None if input is None else subprocess.PIPE
        self._awaitable_process = subprocess.create_subprocess_exec(
            *cmd_to_run,
            cwd=working_dir,
            env=env,
            stdin=stdin,
            stdout=stdout,
            stderr=stderr,
        )

    async def _get_result_or_raise_exception(self, process: subprocess.Process) -> R:
        stdout, stderr = await process.communicate(input=self.input)
        args = (
            process,
            (
                str(stdout, self._encoding)
                if stdout is not None
                else "<stdout not captured>"
            ),
            (
                str(stderr, self._encoding)
                if stderr is not None
                else "<stderr not captured>"
            ),
        )
        if process.returncode != 0:
            raise self._exception_type(
                self.cmd_to_run, self.working_dir, self.env, *args
            )
        return self._result_type(*args)

    async def _wait(self) -> R:
        """Returns a BuckResult with a finished process"""
        process = await self._awaitable_process
        return await self._get_result_or_raise_exception(process)

    async def start(self) -> subprocess.Process:
        """
        Starts a running process and then returns that process.
        Unlike wait, which waits for the process to finish, this does
        not wait for the process to finish.
        """
        return await self._awaitable_process

    def __await__(self) -> Generator[Any, None, R]:
        """
        Overrides __await__ of Awaitable class to implement Awaitable[R].
        Usage:
            ```
            buck_result = await process
            ```
        """
        return self._wait().__await__()

    async def interrupt(self) -> R:
        """Sends SIGINT, and returns a BuckResult with an interrupted process"""
        process = await self._awaitable_process
        process.send_signal(signal.SIGINT)
        return await self._get_result_or_raise_exception(process)
