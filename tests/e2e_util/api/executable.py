#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import platform
from asyncio import subprocess
from enum import Enum
from pathlib import Path
from typing import Dict, List, Optional

from buck2.tests.e2e_util.api.process import Process
from buck2.tests.e2e_util.api.result import Result


class WindowsCmdOption(Enum):
    DelayedExpansion = "v"
    Extensions = "e"


class Executable:
    """An object with a exectuable path"""

    def __init__(
        self,
        path_to_executable: Path,
        encoding: str,
        env: Dict[str, str],
        cwd: Optional[Path] = None,
    ) -> None:
        self.path_to_executable = path_to_executable
        self.cwd = Path() if cwd is None else cwd
        assert self.cwd.exists(), str(self.cwd)
        self.encoding = encoding
        self._env = env
        self._windows_cmd_options: Dict[WindowsCmdOption, bool] = {}

    def _get_command_env(self, env: Optional[Dict[str, str]]) -> Dict[str, str]:
        # Combine self._env and env into 1 dictionary. env overrides self._env
        return {**self._env, **(env or {})}

    def set_env(self, key: str, value: str) -> None:
        self._env[key] = value

    def get_env_var(self, key: str) -> Optional[str]:
        return self._env.get(key)

    def set_windows_cmd_option(self, key: WindowsCmdOption, value: bool) -> None:
        self._windows_cmd_options[key] = value

    def _get_windows_cmd_options(self) -> List[str]:
        """CMD.EXE on windows has two options "DELAYEDEXPANSION" and "EXTENSIONS" that modify the way its scripts are parsed.
        These can be turned on/off with the `/[ve]:<on/off>` arguments to cmd.exe. See the help for `setlocal` for more details
        """
        cmd: List[str] = []
        is_windows = platform.system() == "Windows"
        if is_windows and self._windows_cmd_options:
            cmd.append("cmd.exe")
            for option, value in self._windows_cmd_options.items():
                cmd.append(f"/{option.value}:{'on' if value else 'off'}")
            cmd.append("/c")
        return cmd

    def execute(
        self,
        *argv: str,
        env: Optional[Dict[str, str]] = None,
        input: Optional[bytes] = None,
        stdin: Optional[int] = None,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    ) -> Process[Result, Exception]:
        """
        Runs the executable with a list of arguments.

        argv: a list of arguments to pass to the executable
        env: An optional dictionary of environment variables to run with in addition
        to env passed in constructor. This env overrides env passed in constructor.
        """
        command_env = self._get_command_env(env)
        cmd_to_run = self._get_windows_cmd_options() + [
            str(self.path_to_executable),
            *argv,
        ]

        return Process(
            cmd_to_run=cmd_to_run,
            working_dir=self.cwd,
            env=command_env,
            input=input,
            stdin=stdin,
            stdout=stdout,
            stderr=stderr,
            result_type=Result,
            exception_type=Exception,
            encoding=self.encoding,
        )
