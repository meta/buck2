#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import platform
import re
import stat
from pathlib import Path
from typing import Awaitable, Optional, Type, TypeVar, Union

from buck2.tests.e2e_util.api.buck_result import (
    BuckException,
    BuckResult,
    ExitCode,
    ExitCodeV2,
)


E = TypeVar("E", bound=BaseException)


def assert_executable(output: Path) -> None:
    # stat.S_IXUSR is executable by owner
    # Checks that the file is executable by owner
    # Windows doesn't have executable permission.
    if platform.system() != "Windows":
        assert output.stat().st_mode & stat.S_IXUSR != 0


def assert_not_executable(output: Path) -> None:
    # Checks that the file is not executable by owner
    if platform.system() != "Windows":
        assert output.stat().st_mode & stat.S_IXUSR == 0


def _indent(text: str) -> str:
    return "".join(f"  {line}\n" for line in text.splitlines())


async def expect_failure(
    process: Awaitable[BuckResult],
    *,
    exception: Type[E] = BuckException,
    exit_code: Union[ExitCode, ExitCodeV2, None] = None,
    stdout_regex: Optional[str] = None,
    stderr_regex: Optional[str] = None,
) -> E:
    """
    Asserts that the process raises a BuckException.

    Parameters:
        process: An Awaitable of BuckResult, usually a Process
        exception:
            The type of exception to check for.
            The exception can be a BuckException or any subclass.
            Default is BuckException.
        exit_code:
            An optional exit code to check for if provided.
            Raises an AssertionError if the actual exit code is different.
        stdout_regex:
            An optional regex pattern to search for in stdout if provided.
            Raises an AssertionError if the regex pattern is not found.
        stderr_regex:
            An optional regex pattern to search for in stderr if provided.
            Raises an AssertionError if the regex pattern is not found.
    """
    try:
        await process
    except exception as e:
        failure = e
    else:
        raise AssertionError(f"{exception.__name__} was not raised")
    if not isinstance(failure, BuckException):
        return failure
    if exit_code is not None:
        actual_exit_code = (
            failure.get_exit_code()
            if isinstance(exit_code, ExitCode)
            else failure.get_exit_code_v2()
        )
        assert actual_exit_code == exit_code, (
            f"Expected exit code {exit_code} but found {actual_exit_code}\n<stderr>\n{_indent(failure.stderr)}</stderr>"
        )
    if stdout_regex is not None:
        assert re.search(stdout_regex, failure.stdout, re.DOTALL), (
            f'Did not find pattern: "{stdout_regex}" in stdout: "{failure.stdout}"'
        )
    if stderr_regex is not None:
        assert re.search(stderr_regex, failure.stderr, re.DOTALL | re.IGNORECASE), (
            f'Did not find pattern: "{stderr_regex}" in stderr: "{failure.stderr}"'
        )
    return failure
