#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import os
import uuid
from asyncio import subprocess
from pathlib import Path
from typing import Any, Dict, Iterable, Optional, Tuple

from buck2.tests.e2e_util.api.buck_result import (
    AuditConfigResult,
    BuckException,
    BuckResult,
    BuildResult,
    TargetsResult,
    TestResult,
)
from buck2.tests.e2e_util.api.executable import Executable
from buck2.tests.e2e_util.api.lsp import LspClient
from buck2.tests.e2e_util.api.process import Process
from buck2.tests.e2e_util.api.result import R, Result
from buck2.tests.e2e_util.api.subscribe import SubscribeClient


class Buck(Executable):
    """Instantiates a Buck object with a executable path"""

    def __init__(
        self,
        path_to_executable: Path,
        encoding: str,
        env: Dict[str, str],
        cwd: Optional[Path] = None,
        isolation_prefix: Optional[str] = None,
        write_invocation_record: bool = False,
    ) -> None:
        super().__init__(path_to_executable, encoding, env, cwd)
        self.set_buckd(False)
        self.isolation_prefix = isolation_prefix
        self.write_invocation_record = write_invocation_record

    def set_buckd(self, toggle: bool) -> None:
        """
        Setting buckd env to value of toggle.
        toggle can be 0 for enabled and 1 for disabled
        """
        self._env["NO_BUCKD"] = str(int(toggle))

    def set_isolation_prefix(self, isolation_prefix: str) -> None:
        self.isolation_prefix = isolation_prefix

    def _get_cwd(self, rel_cwd: Optional[Path]) -> Path:
        if rel_cwd is None:
            return self.cwd
        abs_cwd = self.cwd / rel_cwd
        assert abs_cwd.exists(), f"{abs_cwd} doesn't exist"
        return abs_cwd

    def build(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
        stdin: Optional[int] = None,
    ) -> Process[BuildResult, BuckException]:
        """
        Returns a Process with BuildResult type using a process
        created with the build command and any
        additional arguments.

        rel_cwd: Optional Path specifying the working directive to run
        the command relative to the root.
        env: Optional dictionary for environment variables to run command with.
        """
        args = list(argv)
        if not any(arg.startswith("--build-report") for arg in args):
            # For `build` commands, anything after `--` is a positional arg.
            # Find the position of "--" separator to insert --build-report before it
            separator_idx = args.index("--") if "--" in args else len(args)
            args.insert(separator_idx, "--build-report=-")

        return self._run_buck_command(
            "build",
            *args,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
            result_type=BuildResult,
            stdin=stdin,
        )

    def build_without_report(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        """
        Returns a Process with BuckResult type using a process
        created with the build command and any
        additional arguments.

        rel_cwd: Optional Path specifying the workding directive to run
        the command relative to the root.
        env: Optional dictionary for environment variables to run command with.
        """

        return self._run_buck_command(
            "build",
            *argv,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def help(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        return self._run_buck_command(
            "help",
            *argv,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def help_env(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        return self._run_buck_command(
            "help-env",
            *argv,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def run(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        """
        Returns a Process with BuckResult type using a process
        created with the build command and any
        additional arguments

        rel_cwd: Optional Path specifying the workding directive to run
        the command relative to the root.
        env: Optional dictionary for environment variables to run command with.
        """
        return self._run_buck_command(
            "run",
            *argv,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def clean(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        """
        Returns a Process with BuckResult type using a process
        created with the clean command and any
        additional arguments

        rel_cwd: Optional Path specifying the workding directive to run
        the command relative to the root.
        env: Optional dictionary for environment variables to run command with.
        """
        return self._run_buck_command(
            "clean",
            *argv,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def root(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        """
        Returns a Process with BuckResult type using a process
        created with the root command

        kind: --kind argument to the root command
        rel_cwd: Optional Path specifying the workding directory to run
        the command relative to the root.
        env: Optional dictionary for environment variables to run command with.
        """
        return self._run_buck_command(
            "root",
            *argv,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def kill(
        self,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        """
        Returns a Process with BuckResult type using a process
        created with the kill command

        rel_cwd: Optional Path specifying the workding directive to run
        the command relative to the root.
        env: Optional dictionary for environment variables to run command with.
        """
        return self._run_buck_command(
            "kill",
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def test(
        self,
        *argv: str,
        test_executor: Optional[str] = None,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[TestResult, BuckException]:
        """
        Returns a Process with TestResult type using a process
        created with the test command and any
        additional arguments

        rel_cwd: Optional Path specifying the workding directive to run
        the command relative to the root.
        env: Optional dictionary for environment variables to run command with.
        """
        xml_flag, test_output_file = self._create_xml_file()

        argv_list = list(argv)
        argv_separator_idx = (
            argv_list.index("--") if "--" in argv_list else len(argv_list)
        )
        buck_argv = argv_list[0:argv_separator_idx]
        test_argv = argv_list[argv_separator_idx + 1 :]

        if test_executor is None:
            test_executor = os.environ.get("BUCK2_TPX")

        if test_executor is not None:
            buck_argv = [
                "--config",
                "test.v2_test_executor={}".format(test_executor),
                *buck_argv,
            ]

        # Ignore disabled test status if using tpx.
        if test_executor is None or "tpx" in test_executor:
            test_argv += ["--run-disabled"]

        patched_argv = buck_argv + ["--"] + test_argv

        return self._run_buck_command(
            "test",
            *xml_flag,
            *patched_argv,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
            result_type=TestResult,
            result_kwargs={"test_output_file": self.cwd / test_output_file},
        )

    def targets(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[TargetsResult, BuckException]:
        """
        Returns a Process with BuckResult type using a process
        created with the targets command and any
        additional arguments

        rel_cwd: Optional Path specifying the workding directive to run
        the command relative to the root.
        env: Optional dictionary for environment variables to run command with.

        TODO: Add a TargetsResult with structured output.
        """

        return self._run_buck_command(
            "targets",
            *argv,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
            result_type=TargetsResult,
        )

    def ctargets(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        return self._run_buck_command(
            "ctargets",
            *argv,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def complete(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        """
        Returns a Process with BuckResult type using a process
        created with the complete command and any
        additional arguments.

        rel_cwd: Optional Path specifying the workding directive to run
        the command relative to the root.
        env: Optional dictionary for environment variables to run command with.
        """

        my_env = {} if env is None else env.copy()
        my_env["BUCK2_COMPLETION_TIMEOUT"] = "30000"

        return self._run_buck_command(
            "complete",
            *argv,
            input=input,
            rel_cwd=rel_cwd,
            env=my_env,
        )

    def completion(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        """
        Returns a Process with BuckResult type using a process
        created with the completion command and any
        additional arguments.

        rel_cwd: Optional Path specifying the workding directive to run
        the command relative to the root.
        env: Optional dictionary for environment variables to run command with.
        """
        return self._run_buck_command(
            "completion",
            *argv,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def audit_config(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[AuditConfigResult, BuckException]:
        """
        Returns a Process with AuditConfigResult type using a process
        created with the audit_config command

        rel_cwd: Optional Path specifying the workding directive to run
        the command relative to the root.
        env: Optional dictionary for environment variables to run command with.
        """
        args = list(argv)
        return self._run_buck_command(
            "audit",
            "config",
            *args,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
            result_type=AuditConfigResult,
        )

    def audit_configurations(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        args = list(argv)
        return self._run_buck_command(
            "audit",
            "configurations",
            *args,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def audit_dep_files(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        args = list(argv)
        return self._run_buck_command(
            "audit",
            "dep-files",
            *args,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def audit_visibility(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        args = list(argv)
        return self._run_buck_command(
            "audit",
            "visibility",
            *args,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def audit(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        args = list(argv)
        return self._run_buck_command(
            "audit",
            *args,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def audit_output(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        args = list(argv)
        return self._run_buck_command(
            "audit",
            "output",
            *args,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def query(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        return self._query("query", *argv, rel_cwd=rel_cwd, env=env)

    def cquery(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        return self._query("cquery", *argv, rel_cwd=rel_cwd, env=env)

    def uquery(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        return self._query("uquery", *argv, rel_cwd=rel_cwd, env=env)

    def aquery(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        return self._query("aquery", *argv, rel_cwd=rel_cwd, env=env)

    def _query(
        self,
        query_command: str,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        """
        Returns a Process with BuckResult type using a process
        created with the query command and any
        additional arguments

        rel_cwd: Optional Path specifying the workding directive to run
        the command relative to the root.
        env: Optional dictionary for environment variables to run command with.

        TODO: Add a QueryResult with structured output.
        """
        return self._run_buck_command(
            query_command,
            *argv,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def bxl(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        args = list(argv)
        return self._run_buck_command(
            "bxl",
            *args,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def docs(
        self,
        *args: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        return self._run_buck_command(
            "docs",
            *args,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def profile(
        self,
        *args: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        """
        Returns a Process with BuckResult type using a process created with the
        profile command and any additional arguments

        args: Arguments to pass to buck2 profile.
        rel_cwd: Optional Path specifying the workding directive to run
        the command relative to the root.
        env: Optional dictionary for environment variables to run command with.
        """
        return self._run_buck_command(
            "profile",
            *args,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def debug(
        self,
        *args: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        """
        Returns a Process with BuckResult type using a process created with the
        debug command and any additional arguments
        """
        return self._run_buck_command(
            "debug",
            *args,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def starlark(
        self,
        *args: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        """
        Returns a Process with BuckResult type using a process created with the
        debug command and any additional arguments
        """
        return self._run_buck_command(
            "starlark",
            *args,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def install(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        return self._run_buck_command(
            "install",
            *argv,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def log(
        self,
        *args: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        return self._run_buck_command(
            "log",
            *args,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
            can_write_invocation_record=False,
        )

    def status(
        self,
        *args: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        return self._run_buck_command(
            "status",
            *args,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
            can_write_invocation_record=False,
        )

    def server(
        self,
        *args: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        return self._run_buck_command(
            "server",
            *args,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def expand_external_cell(
        self,
        *args: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        return self._run_buck_command(
            "expand-external-cell",
            *args,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    async def lsp(
        self,
        *args: str,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> LspClient:
        process = await self._run_buck_command(
            "lsp",
            *args,
            input=None,
            stdin=subprocess.PIPE,
            rel_cwd=rel_cwd,
            env=env,
            intercept_stderr=False,
        ).start()
        cwd = self._get_cwd(rel_cwd)
        return LspClient(process, cwd)

    async def subscribe(
        self,
        *args: str,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> SubscribeClient:
        process = self._run_buck_command(
            "subscribe",
            "--unstable-json",
            *args,
            input=None,
            stdin=subprocess.PIPE,
            rel_cwd=rel_cwd,
            env=env,
            intercept_stderr=False,
        )
        client = await SubscribeClient.create(process)
        return client

    def construct_buck_command(
        self,
        cmd: str,
        *argv: str,
    ) -> list[str]:
        """
        Returns a list of strings representing the buck command
        """
        cmd_to_run = [str(self.path_to_executable), cmd]
        if self.isolation_prefix:
            cmd_to_run = [
                cmd_to_run[0],
                "--isolation-dir",
                str(self.isolation_prefix),
                *cmd_to_run[1:],
            ]
        cmd_to_run.extend(argv)
        cmd_to_run = self._get_windows_cmd_options() + cmd_to_run
        return cmd_to_run

    def _run_buck_command(
        self,
        cmd: str,
        *argv: str,
        input: Optional[bytes],
        rel_cwd: Optional[Path],
        env: Optional[Dict[str, str]],
        result_type: type[R] = BuckResult,
        result_kwargs: Optional[Dict[str, Any]] = None,
        stdin: Optional[int] = None,
        intercept_stderr: bool = True,
        can_write_invocation_record: bool = True,
    ) -> Process[R, BuckException]:
        """
        Returns a process created from the execuable path,
        command and any additional arguments
        """
        invocation_id = str(uuid.uuid1())
        command_env = self._get_command_env(env)
        if "BUCK_WRAPPER_UUID" not in command_env:
            command_env["BUCK_WRAPPER_UUID"] = invocation_id

        cwd = self._get_cwd(rel_cwd)

        args = list(argv)
        invocation_record_path = None
        if self.write_invocation_record and can_write_invocation_record:
            invocation_record_dir = cwd / "buck-out" / "tmp"
            invocation_record_dir.mkdir(parents=True, exist_ok=True)
            invocation_record_path = invocation_record_dir / (invocation_id + ".json")
            separator_idx = args.index("--") if "--" in args else len(args)
            args[separator_idx:separator_idx] = [
                "--unstable-write-invocation-record",
                str(invocation_record_path),
            ]

        cmd_to_run = self.construct_buck_command(cmd, *args)

        args_tuple: Tuple[str, ...] = argv
        result_kwargs = result_kwargs or {}
        _invocation_record_path: Optional[Path] = invocation_record_path

        def make_result(proc: subprocess.Process, stdout: str, stderr: str) -> R:
            base = BuckResult(
                proc,
                stdout,
                stderr,
                _invocation_record_path,
                buck_args=" ".join(args_tuple),
            )
            if result_type is BuckResult:
                return base  # type: ignore[return-value]
            return result_type(base, **result_kwargs)  # type: ignore[return-value]

        _invocation_record_path2: Optional[Path] = invocation_record_path

        def make_exception(
            cmd_to_run: str,
            working_dir: Path,
            env: Dict[str, str],
            proc: subprocess.Process,
            stdout: str,
            stderr: str,
        ) -> BuckException:
            return BuckException(
                cmd_to_run,
                working_dir,
                env,
                proc,
                stdout,
                stderr,
                _invocation_record_path2,
            )

        stderr = subprocess.PIPE if intercept_stderr else None
        return Process(
            cmd_to_run=cmd_to_run,
            working_dir=cwd,
            env=command_env,
            input=input,
            stdin=stdin,
            stdout=subprocess.PIPE,
            stderr=stderr,
            result_type=make_result,
            # pyrefly: ignore [bad-argument-type]
            exception_type=make_exception,
            encoding=self.encoding,
        )

    def run_buck_command(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        return self._run_buck_command(
            *argv,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def _create_xml_file(self, *argv: str) -> Tuple[Iterable[str], str]:
        """
        Creates a xml file used for the test output. Ensures an xml file
        is created if not specified.
        """
        xml_flag = [""]
        test_output_file = "testOutput.xml"
        # ensures xml file is always generated
        if "--xml" not in argv:
            xml_flag = ["--xml", "testOutput.xml"]
        else:
            test_output_file = argv[argv.index("--xml") + 1]
        return xml_flag, test_output_file

    def execute(
        self,
        *argv: str,
        env: Optional[Dict[str, str]] = None,
        input: Optional[bytes] = None,
        stdin: Optional[int] = None,
        stdout: int = subprocess.PIPE,
        stderr: int = subprocess.PIPE,
    ) -> Process[Result, Exception]:
        raise NotImplementedError("Buck does not use execute.")

    def rage(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        return self._run_buck_command(
            "rage",
            *argv,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def explain(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        return self._run_buck_command(
            "explain",
            *argv,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    def init(
        self,
        *argv: str,
        input: Optional[bytes] = None,
        rel_cwd: Optional[Path] = None,
        env: Optional[Dict[str, str]] = None,
    ) -> Process[BuckResult, BuckException]:
        return self._run_buck_command(
            "init",
            *argv,
            input=input,
            rel_cwd=rel_cwd,
            env=env,
        )

    async def get_daemon_dir(self) -> Path:
        # `debug daemon-dir` doesn't accept `--unstable-write-invocation-record`.
        daemon_dir = await self._run_buck_command(
            "debug",
            "daemon-dir",
            input=None,
            rel_cwd=None,
            env=None,
            can_write_invocation_record=False,
        )
        return Path(daemon_dir.stdout.strip())

    async def daemon_stderr(self) -> str:
        daemon_dir = await self.get_daemon_dir()
        return (daemon_dir / "buckd.stderr").read_text()

    async def prev_daemon_stderr(self) -> str:
        daemon_dir = await self.get_daemon_dir()
        return (daemon_dir / "prev/buckd.stderr").read_text()

    def get_settings_home_dir(self) -> Path:
        return Path(self._env["BUCK2_TEST_SETTINGS_HOME_DIR"])
