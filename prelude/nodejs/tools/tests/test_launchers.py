# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import contextlib
import io
import json
import os
import re
import shlex
import shutil
import stat
import string
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from package_tree import main, write_launchers

# Values with each character that sh, cmd.exe or the Microsoft C runtime treats specially.
QUOTING_TABLE = [
    "plain",
    "",
    "a b",
    "  ",
    "\t",
    "100%",
    "%PATH%",
    "%~dp0",
    "%%",
    "%1",
    "%*",
    "x^y",
    "^",
    "^^",
    "&",
    "a&b",
    "|",
    "<",
    ">",
    "(x)",
    ")",
    "!bang!",
    "!",
    'q"uote',
    '"',
    '""',
    'a\\"b',
    'a\\\\"b',
    "back\\slash\\",
    "\\",
    "C:\\Program Files\\",
    "'",
    "it's",
    "a'\\",
    "$HOME",
    "${HOME}",
    "`id`",
    "$( id)",
    "*",
    "?",
    "~",
    "#x",
    ";",
    "a,b",
    "a=b",
    "--k=v",
    "@x",
    "-",
    "--",
    "/?",
    "x/?",
    "{app_root}",
    "{app_root}/x",
    "{app_root}\\",
    'a"{app_root}',
    '"{app_root}&',
    "pre{app_root}post",
    "{app_root}{app_root}",
    "%{app_root}%~R%R",
]


def _spec(kind: str = "native", **fields: object) -> dict[str, object]:
    runtime: dict[str, object] = {"kind": kind}
    if kind != "host":
        runtime["posix_exe"] = "node"
        runtime["windows_exe"] = "node.bat" if kind == "pointer" else "node.exe"
    spec: dict[str, object] = {
        "launchers": [{"name": "app", "entry": "lib/main.js"}],
        "runtime": runtime,
    }
    spec.update(fields)
    return spec


def _table_env() -> dict[str, str]:
    return {f"V{i}": value for i, value in enumerate(QUOTING_TABLE) if value}


def paren_depths(script: str) -> list[int]:
    """The nesting depth after each parenthesis outside quotes in `script`.

    bash 3.2, macOS's sh, ends a `$(` at the `)` that balances it this way, so a depth below zero
    means that a `$(` ends early, as it would at a case pattern's `)`.
    """
    depths = []
    quote = None
    chars = iter(script)
    for c in chars:
        if c == "\\" and quote != "'":
            next(chars, None)
        elif quote:
            quote = None if c == quote else quote
        elif c in "'\"":
            quote = c
        elif c in "()":
            depths.append((depths[-1] if depths else 0) + (1 if c == "(" else -1))
    return depths


@unittest.skipIf(sys.platform == "win32", "runs the sh launchers")
class TestShLaunchers(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(os.path.realpath(tmp.name))
        # Characters that sh would expand, split or glob, and the separator `ls -l` prints.
        self.root = self.tmp / "it's $HOME -> `x` *"
        self.env = {
            name: value
            for name, value in os.environ.items()
            if name not in ("CDPATH", "NODEJS_LAUNCHER_NODE")
        }

    def _fake_node(self, path: Path, name: str) -> None:
        code = (
            "import json, os, sys; "
            "json.dump({'node': sys.argv[1], 'argv': sys.argv[2:], 'cwd': os.getcwd(), "
            "'env': dict(os.environ)}, sys.stdout); "
            "sys.exit(int(os.environ.get('FAKE_NODE_EXIT', '0')))"
        )
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            f'#!/bin/sh\nexec {shlex.quote(sys.executable)} -c {shlex.quote(code)} {name} "$@"\n'
        )
        path.chmod(0o755)

    def _run(
        self, command: list[str], cwd: Path | None = None, **env: str
    ) -> dict[str, object]:
        result = subprocess.run(
            command,
            cwd=cwd,
            env={**self.env, **env},
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def _expand(self, value: str) -> str:
        return value.replace("{app_root}", str(self.root))

    def test_quoting_table(self) -> None:
        spec = _spec(node_args=QUOTING_TABLE, args=QUOTING_TABLE, env=_table_env())
        spec["runtime"]["node_args"] = ["--runtime-arg"]
        write_launchers(spec, self.root)
        self._fake_node(self.root / "node", "bundled")
        user_args = ["user arg", "", "{app_root}", "$HOME", "'", '"', "\\"]
        out = self._run([str(self.root / "bin" / "app"), *user_args], self.tmp)
        self.assertEqual(out["cwd"], str(self.tmp))
        table = [self._expand(value) for value in QUOTING_TABLE]
        entry = f"{self.root}/lib/main.js"
        self.assertEqual(
            out["argv"], ["--runtime-arg", *table, entry, *table, *user_args]
        )
        self.assertEqual(
            {name: out["env"].get(name) for name in _table_env()},
            {name: self._expand(value) for name, value in _table_env().items()},
        )

    def test_runs_through_symlinks(self) -> None:
        write_launchers(_spec(), self.root)
        self._fake_node(self.root / "node", "bundled")
        launcher = self.root / "bin" / "app"
        links = self.tmp / "links"
        (links / "a").mkdir(parents=True)
        os.symlink(launcher, links / "absolute")
        os.symlink(os.path.relpath(launcher, links / "a"), links / "a" / "relative")
        os.symlink("a/relative", links / "chain")
        os.symlink(self.root, self.tmp / "root link", target_is_directory=True)
        path = f"{links}:{os.environ['PATH']}"
        cases = {
            "absolute symlink": ([str(links / "absolute")], None, {}),
            "relative symlink chain": (["./chain"], links, {}),
            "relative path to the chain": (["links/chain"], self.tmp, {}),
            "sh and the chain": (["sh", "links/chain"], self.tmp, {}),
            "chain from PATH": (["chain"], None, {"PATH": path}),
            "symlinked root": ([str(self.tmp / "root link" / "bin" / "app")], None, {}),
        }
        for case, (command, cwd, env) in cases.items():
            with self.subTest(case):
                out = self._run(command, cwd, **env)
                self.assertEqual(out["argv"], [f"{self.root}/lib/main.js"])
                # GNU and BSD `ls` quote or color names for these even when not on a terminal.
                quoting = {"QUOTING_STYLE": "shell-escape", "CLICOLOR_FORCE": "1"}
                out = self._run(command, cwd, **env, **quoting)
                self.assertEqual(out["argv"], [f"{self.root}/lib/main.js"])

    def test_ignores_cdpath(self) -> None:
        write_launchers(_spec(), self.root)
        self._fake_node(self.root / "node", "bundled")
        decoy = self.tmp / "decoy"
        (decoy / "bin").mkdir(parents=True)
        out = self._run(["bin/app"], self.root, CDPATH=str(decoy))
        self.assertEqual(out["argv"], [f"{self.root}/lib/main.js"])

    def test_posix_shells(self) -> None:
        shells = [
            shell
            for shell in (
                ["dash"],
                ["bash", "--posix"],
                ["ksh"],
                ["mksh"],
                ["zsh", "--emulate", "sh"],
                ["busybox", "sh"],
            )
            if shutil.which(shell[0])
        ]
        if not shells:
            self.skipTest("no POSIX shell besides sh is installed")
        write_launchers(_spec(args=["a b"]), self.root)
        self._fake_node(self.root / "node", "bundled")
        os.symlink(self.root / "bin" / "app", self.tmp / "link")
        for shell in shells:
            with self.subTest(shell[0]):
                out = self._run([*shell, str(self.tmp / "link"), "c d"])
                self.assertEqual(
                    out["argv"], [f"{self.root}/lib/main.js", "a b", "c d"]
                )

    def test_runtime_kinds(self) -> None:
        path_dir = self.tmp / "path"
        self._fake_node(path_dir / "node", "path")
        self._fake_node(self.tmp / "override", "override")
        path = f"{path_dir}:{os.environ['PATH']}"
        for kind, posix_exe, node in (
            ("native", "node", "native"),
            ("pointer", "runtime/node", "pointer"),
            ("host", None, "path"),
        ):
            with self.subTest(kind):
                spec = _spec(kind)
                if posix_exe:
                    spec["runtime"]["posix_exe"] = posix_exe
                root = self.tmp / kind
                write_launchers(spec, root)
                if posix_exe:
                    self._fake_node(root / posix_exe, node)
                launcher = [str(root / "bin" / "app")]
                self.assertEqual(self._run(launcher, PATH=path)["node"], node)
                self.assertEqual(
                    self._run(launcher, PATH=path, NODEJS_LAUNCHER_NODE="")["node"],
                    node,
                )
                override = str(self.tmp / "override")
                self.assertEqual(
                    self._run(launcher, PATH=path, NODEJS_LAUNCHER_NODE=override)[
                        "node"
                    ],
                    "override",
                )

    def test_env(self) -> None:
        spec = _spec(env={"BOTH": "target", "CLEARED": "", "ROOTED": "{app_root}/x"})
        spec["runtime"]["env"] = {"BOTH": "runtime", "RUNTIME": "runtime"}
        write_launchers(spec, self.root)
        self._fake_node(self.root / "node", "bundled")
        out = self._run([str(self.root / "bin" / "app")], CLEARED="inherited")
        self.assertEqual(
            {
                name: out["env"].get(name)
                for name in ("BOTH", "RUNTIME", "CLEARED", "ROOTED")
            },
            {
                "BOTH": "target",
                "RUNTIME": "runtime",
                "CLEARED": None,
                "ROOTED": f"{self.root}/x",
            },
        )

    def test_exit_status(self) -> None:
        write_launchers(_spec(), self.root)
        self._fake_node(self.root / "node", "bundled")
        result = subprocess.run(
            [str(self.root / "bin" / "app")],
            env={**self.env, "FAKE_NODE_EXIT": "3"},
            capture_output=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 3)

    def test_one_pair_per_launcher(self) -> None:
        launchers = [
            {"name": "one", "entry": "a/one.js"},
            {"name": "two", "entry": "b/two.js"},
        ]
        write_launchers(_spec(launchers=launchers), self.root)
        self._fake_node(self.root / "node", "bundled")
        self.assertEqual(
            sorted(os.listdir(self.root / "bin")), ["one", "one.cmd", "two", "two.cmd"]
        )
        out = self._run([str(self.root / "bin" / "two")])
        self.assertEqual(out["argv"], [f"{self.root}/b/two.js"])

    def test_files_ignore_umask(self) -> None:
        umask = os.umask(0o077)
        try:
            write_launchers(_spec(), self.root)
        finally:
            os.umask(umask)
        sh, cmd = self.root / "bin" / "app", self.root / "bin" / "app.cmd"
        self.assertEqual(stat.S_IMODE(sh.stat().st_mode), 0o755)
        self.assertEqual(stat.S_IMODE(cmd.stat().st_mode), 0o644)

    def test_line_endings(self) -> None:
        write_launchers(_spec(), self.root)
        sh, cmd = self.root / "bin" / "app", self.root / "bin" / "app.cmd"
        self.assertNotIn(b"\r", sh.read_bytes())
        lines = cmd.read_bytes().split(b"\r\n")
        self.assertEqual(lines[-1], b"")
        self.assertFalse(any(b"\n" in line or b"\r" in line for line in lines))

    def test_parens_balance(self) -> None:
        for kind in ("native", "host"):
            with self.subTest(kind):
                spec = _spec(
                    kind, node_args=QUOTING_TABLE, args=QUOTING_TABLE, env=_table_env()
                )
                write_launchers(spec, self.tmp / kind)
                depths = paren_depths((self.tmp / kind / "bin" / "app").read_text())
                self.assertGreaterEqual(min(depths), 0)
                self.assertEqual(depths[-1], 0)

    @unittest.skipUnless(shutil.which("shellcheck"), "shellcheck is not installed")
    def test_shellcheck(self) -> None:
        for kind in ("native", "host"):
            with self.subTest(kind):
                spec = _spec(
                    kind, node_args=QUOTING_TABLE, args=QUOTING_TABLE, env=_table_env()
                )
                write_launchers(spec, self.tmp / kind)
                result = subprocess.run(
                    ["shellcheck", "--shell=sh", str(self.tmp / kind / "bin" / "app")],
                    capture_output=True,
                    text=True,
                    timeout=30,
                )
                self.assertEqual(result.returncode, 0, result.stdout)


def split_like_msvcrt(command_line: str) -> list[str]:
    """Splits arguments like the Microsoft C runtime (Visual C++ 2008 and later)."""
    args = []
    i = 0
    while True:
        while command_line[i : i + 1] in (" ", "\t"):
            i += 1
        if i == len(command_line):
            return args
        arg = []
        quoted = False
        while i < len(command_line):
            backslashes = len(command_line[i:]) - len(command_line[i:].lstrip("\\"))
            i += backslashes
            if command_line[i : i + 1] == '"':
                arg.append("\\" * (backslashes // 2))
                if backslashes % 2:
                    arg.append('"')
                elif quoted and command_line[i + 1 : i + 2] == '"':
                    arg.append('"')
                    i += 1
                else:
                    quoted = not quoted
                i += 1
                continue
            arg.append("\\" * backslashes)
            if i == len(command_line) or (not quoted and command_line[i] in " \t"):
                break
            arg.append(command_line[i])
            i += 1
        args.append("".join(arg))


class CmdModel:
    """Runs a batch launcher the way cmd.exe does, with delayed expansion off.

    A line is percent-expanded, with `%NAME:OLD=NEW%` replacing OLD without regard to case and
    `%NAME:~START,LENGTH%` taking a substring, then parsed: `"` toggles quoting, and outside quotes `^` escapes the next character and
    `&|<>()` are operators. `call` fails on an unquoted `/?`, which makes it print its help, and
    then doubles every caret, quoted ones included, and expands and parses the line again. FOR
    substitutes its variable after parsing. cmd.exe gives the launchers this file tests the same
    results.
    """

    def __init__(
        self, script: str, script_dir: str, env: dict[str, str], args: str
    ) -> None:
        self.lines = script.split("\r\n")
        self.script_dir = script_dir
        self.env = {name.upper(): value for name, value in env.items()}
        self.args = args

    def expand(self, line: str) -> str:
        out = []
        i = 0
        while i < len(line):
            if line[i] != "%":
                out.append(line[i])
                i += 1
                continue
            for token, value in (
                ("%%", "%"),
                ("%*", self.args),
                ("%~dp0", self.script_dir),
            ):
                if line.startswith(token, i):
                    out.append(value)
                    i += len(token)
                    break
            else:
                end = line.find("%", i + 1)
                if end < 0:
                    i += 1
                    continue
                name, colon, edit = line[i + 1 : end].partition(":")
                value = self.env.get(name.upper())
                if not colon:
                    out.append(value or "")
                    i = end + 1
                elif value is None:
                    # `%NAME:OLD=NEW%` leaves `OLD=NEW%` when NAME is undefined.
                    i += len(name) + 2
                elif edit.startswith("~"):
                    start, length = map(int, edit[1:].split(","))
                    out.append(value[start : length if length < 0 else start + length])
                    i = end + 1
                else:
                    old, new = edit.split("=", 1)
                    pattern = re.compile(re.escape(old), re.IGNORECASE)
                    out.append(pattern.sub(new.replace("\\", r"\\"), value))
                    i = end + 1
        return "".join(out)

    @staticmethod
    def parse(line: str) -> str:
        out = []
        quoted = False
        chars = iter(line)
        for c in chars:
            if c == '"':
                quoted = not quoted
            elif not quoted and c == "^":
                c = next(chars)
            elif not quoted and c in "&|<>()":
                raise AssertionError(f"cmd.exe would act on {c!r} in {line!r}")
            out.append(c)
        return "".join(out)

    def call(self, line: str) -> str:
        """Runs a `call` line; returns the command line of the program it starts."""
        command = self.parse(self.expand(line)).removeprefix("call ")
        quoted = False
        for i, c in enumerate(command):
            quoted ^= c == '"'
            if not quoted and command.startswith("/?", i):
                raise AssertionError(f"call prints its help for {line!r}")
        return self.parse(self.expand(command.replace("^", "^^")))

    def set(self, command: str) -> None:
        if not command.startswith('set "'):
            raise AssertionError(command)
        name, value = command[len('set "') : command.rindex('"')].split("=", 1)
        if value:
            self.env[name.upper()] = value
        else:
            self.env.pop(name.upper(), None)

    def run(self) -> str:
        """Runs the launcher up to its `call`; returns the command line it runs."""
        if self.lines[:2] != [
            "@echo off",
            "setlocal EnableExtensions DisableDelayedExpansion",
        ]:
            raise AssertionError(self.lines[:2])
        for i, line in enumerate(self.lines[2:], 2):
            if line == "call :root":
                for sub in self.lines[self.lines.index(":root") + 1 :]:
                    if sub == "exit /b":
                        break
                    self.set(self.parse(self.expand(sub)))
            elif line.startswith(("if defined ", "if not defined ")):
                # The whole line is expanded before `if` tests the variable.
                negate = line.startswith("if not ")
                _, *_, name, command = self.parse(self.expand(line)).split(
                    " ", 4 if negate else 3
                )
                if (name.upper() in self.env) != negate:
                    self.set(command)
            elif line.startswith("for %%") and line[7:].startswith(
                ' in ("%__ROOT%") do set "'
            ):
                command = self.parse(self.expand(line[line.index(" do ") + 4 :]))
                self.set(command.replace("%~" + line[6], self.env["__ROOT"]))
            elif line.startswith("set "):
                self.set(self.parse(self.expand(line)))
            elif line.startswith("call "):
                self.assert_tail(self.lines[i + 1 :])
                return self.call(line)
            else:
                raise AssertionError(f"unexpected line {line!r}")
        raise AssertionError("no call line")

    @staticmethod
    def assert_tail(lines: list[str]) -> None:
        if lines != [
            "exit /b %ERRORLEVEL%",
            ":root",
            'set "__ROOT=%~dp0.."',
            "exit /b",
            "",
        ]:
            raise AssertionError(lines)


def forward(args: str) -> str:
    """The arguments that a batch file line `PROGRAM %*` passes PROGRAM when given `args`."""
    model = CmdModel("", "", {}, args)
    return model.parse(model.expand("%*"))


def split_program(command_line: str) -> tuple[str, str]:
    """Splits off the quoted program; the rest is what a batch program gets as `%*`."""
    end = command_line.index('"', 1)
    return command_line[1:end], command_line[end + 1 :].lstrip(" ")


class TestCmdLaunchers(unittest.TestCase):
    SCRIPT_DIR = "C:\\x\\a b&c(d)^e%f!g,h;i=j'k\\app\\bin\\"
    ROOT = SCRIPT_DIR + ".."
    USER_ARGS = '/? plain "a b" "50%" "a^b" "x&y" "q""u" "!b!" "(p)" "c:\\d e\\\\"'

    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.out = Path(tmp.name)

    def _run(
        self, spec: dict[str, object], env: dict[str, str]
    ) -> tuple[str, str, CmdModel]:
        shutil.rmtree(self.out / "bin", ignore_errors=True)
        write_launchers(spec, self.out)
        script = (self.out / "bin" / "app.cmd").read_bytes().decode("ascii")
        model = CmdModel(script, self.SCRIPT_DIR, env, self.USER_ARGS)
        program, args = split_program(model.run())
        return program, args, model

    def test_quoting_table(self) -> None:
        user_args = [
            "/?",
            "plain",
            "a b",
            "50%",
            "a^b",
            "x&y",
            'q"u',
            "!b!",
            "(p)",
            "c:\\d e\\",
        ]
        table = [value.replace("{app_root}", self.ROOT) for value in QUOTING_TABLE]
        argv = [
            "--runtime-arg",
            *table,
            self.ROOT + "\\lib\\main.js",
            *table,
            *user_args,
        ]
        override = "D:\\o&p^q\\node.exe"
        for case, kind, env, node in (
            ("native", "native", {}, self.ROOT + "\\node.exe"),
            ("pointer", "pointer", {}, self.ROOT + "\\node.bat"),
            ("host", "host", {}, "node"),
            ("override", "native", {"NODEJS_LAUNCHER_NODE": override}, override),
            (
                "empty override",
                "native",
                {"NODEJS_LAUNCHER_NODE": ""},
                self.ROOT + "\\node.exe",
            ),
        ):
            with self.subTest(case):
                spec = _spec(
                    kind, node_args=QUOTING_TABLE, args=QUOTING_TABLE, env=_table_env()
                )
                spec["runtime"]["node_args"] = ["--runtime-arg"]
                program, args, model = self._run(spec, env)
                self.assertEqual(program, node)
                self.assertEqual(split_like_msvcrt(args), argv)
                # A runtime such as `node.bat` passes `%*` on in one more batch line.
                self.assertEqual(split_like_msvcrt(forward(args)), argv)
                self.assertEqual(
                    {name: model.env.get(name) for name in _table_env()},
                    {
                        name: value.replace("{app_root}", self.ROOT)
                        for name, value in _table_env().items()
                    },
                )

    def test_env(self) -> None:
        spec = _spec(env={"BOTH": "target", "CLEARED": ""})
        spec["runtime"]["env"] = {"BOTH": "runtime", "RUNTIME": "runtime"}
        _, _, model = self._run(spec, {"Cleared": "inherited"})
        self.assertEqual(
            {name: model.env.get(name) for name in ("BOTH", "RUNTIME", "CLEARED")},
            {"BOTH": "target", "RUNTIME": "runtime", "CLEARED": None},
        )

    def test_host_runtime_drops_js_from_pathext(self) -> None:
        pathext = ".COM;.EXE;.BAT;.CMD;.VBS;.VBE;.js;.JSE;.WSF;.WSH;.MSC"
        for case, spec, env, want in (
            (
                "host",
                _spec("host"),
                {"PATHEXT": pathext},
                ".COM;.EXE;.BAT;.CMD;.VBS;.VBE;.JSE;.WSF;.WSH;.MSC",
            ),
            ("first", _spec("host"), {"PATHEXT": ".Js;.EXE"}, ".EXE"),
            ("last", _spec("host"), {"PATHEXT": ".EXE;.JS"}, ".EXE"),
            ("repeated", _spec("host"), {"PATHEXT": ".JS;.js;.EXE"}, ".EXE"),
            ("empty entry", _spec("host"), {"PATHEXT": ".EXE;;.JS"}, ".EXE;"),
            ("only", _spec("host"), {"PATHEXT": ".JS"}, None),
            ("native", _spec("native"), {"PATHEXT": pathext}, pathext),
            ("pointer", _spec("pointer"), {"PATHEXT": pathext}, pathext),
            ("unset by a calling batch file", _spec("host"), {}, None),
            (
                "set by env",
                _spec("host", env={"PATHEXT": ".EXE;.JS;.CMD"}),
                {"PATHEXT": pathext},
                ".EXE;.CMD",
            ),
        ):
            with self.subTest(case):
                _, _, model = self._run(spec, env)
                self.assertEqual(model.env.get("PATHEXT"), want)


class TestLauncherSpec(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.out = self.tmp / "out"

    def test_rejects(self) -> None:
        def with_runtime(**runtime: object) -> dict[str, object]:
            spec = _spec()
            spec["runtime"] = runtime
            return spec

        def named(*names: str) -> dict[str, object]:
            return _spec(
                launchers=[{"name": name, "entry": "main.js"} for name in names]
            )

        for case, spec, error in (
            ("macro in args", _spec(args=["$(location //a:b)"]), "Buck macro"),
            ("macro in env", _spec(env={"A": "x$(exe :y)"}), "Buck macro"),
            ("written macro", _spec(node_args=["$(@query_outputs :y)"]), "Buck macro"),
            (
                "macro name with a dash",
                _spec(args=["$(location-platform //a:b)"]),
                "Buck macro",
            ),
            ("macro with no args", _spec(env={"A": "$(ldflags-shared)"}), "Buck macro"),
            ("non-ASCII", _spec(args=["caf\u00e9"]), "printable ASCII"),
            ("line break", _spec(env={"A": "a\nb"}), "printable ASCII"),
            ("env name", _spec(env={"A-B": "x"}), "cannot set 'A-B'"),
            ("launcher variable", _spec(env={"__root": "x"}), "cannot set '__root'"),
            (
                "exit code variable",
                _spec(env={"errorlevel": "0"}),
                "cannot set 'errorlevel'",
            ),
            (
                "env names differing in case",
                _spec(env={"Path": "a", "PATH": "b"}),
                "both 'Path' and 'PATH'",
            ),
            (
                "env and runtime env names differing in case",
                {
                    **_spec(env={"Path": "b"}),
                    "runtime": {"kind": "host", "env": {"PATH": "a"}},
                },
                "both 'PATH' and 'Path'",
            ),
            ("name with /", named("a/b"), "has a `/`"),
            ("named node", named("node"), "shadow node"),
            ("named node.exe", named("Node.exe"), "shadow node"),
            ("device name", named("con"), "portable"),
            ("trailing dot", named("app."), "portable"),
            ("colon", named("a:b"), "portable"),
            ("case-insensitive duplicate", named("app", "APP"), "bin/APP twice"),
            ("named x.cmd", named("x.cmd"), "Windows executable extension"),
            ("named x.EXE", named("x.EXE"), "Windows executable extension"),
            ("named x.js", named("x.js"), "Windows executable extension"),
            ("no launchers", _spec(launchers=[]), "non-empty"),
            (
                "parent entry",
                _spec(launchers=[{"name": "a", "entry": "../a.js"}]),
                "portable",
            ),
            (
                "absolute entry",
                _spec(launchers=[{"name": "a", "entry": "/a.js"}]),
                "portable",
            ),
            (
                "backslash entry",
                _spec(launchers=[{"name": "a", "entry": "a\\a.js"}]),
                "portable",
            ),
            ("unknown kind", with_runtime(kind="bundled"), "runtime.kind"),
            ("host exe", with_runtime(kind="host", posix_exe="node"), "host runtime"),
            (
                "missing exe",
                with_runtime(kind="native", posix_exe="node"),
                "windows_exe",
            ),
            (
                "unquotable exe",
                with_runtime(
                    kind="pointer", posix_exe="my node", windows_exe="node.bat"
                ),
                "do not quote",
            ),
            ("unknown key", _spec(arg=["x"]), "unknown keys"),
            (
                "no FOR variable",
                _spec(env={"A": "{app_root}" + string.ascii_letters}),
                "FOR variable",
            ),
        ):
            with self.subTest(case):
                out = Path(tempfile.mkdtemp(dir=self.tmp)) / "out"
                with self.assertRaisesRegex(ValueError, error):
                    write_launchers(spec, out)
                self.assertFalse(out.exists())

    def test_names_equal_to_an_extension(self) -> None:
        launchers = [{"name": name, "entry": "main.js"} for name in ("cmd", "exe")]
        write_launchers(_spec(launchers=launchers), self.out)
        self.assertEqual(
            sorted(os.listdir(self.out / "bin")), ["cmd", "cmd.cmd", "exe", "exe.cmd"]
        )

    def test_names_that_start_with_node(self) -> None:
        launchers = [
            {"name": name, "entry": "main.js"} for name in ("nodemon", "node-gyp")
        ]
        write_launchers(_spec(launchers=launchers), self.out)
        self.assertEqual(
            sorted(os.listdir(self.out / "bin")),
            ["node-gyp", "node-gyp.cmd", "nodemon", "nodemon.cmd"],
        )

    def test_cli(self) -> None:
        spec = self.tmp / "spec.json"
        argv = [
            "package_tree.py",
            "launchers",
            "--spec",
            str(spec),
            "--out",
            str(self.out),
        ]
        spec.write_text(json.dumps(_spec()))
        with mock.patch.object(sys, "argv", argv):
            self.assertEqual(main(), 0)
        self.assertEqual(sorted(os.listdir(self.out / "bin")), ["app", "app.cmd"])
        spec.write_text(json.dumps(_spec(args=["$(exe :x)"])))
        stderr = io.StringIO()
        with mock.patch.object(sys, "argv", argv), contextlib.redirect_stderr(stderr):
            self.assertEqual(main(), 1)
        self.assertIn("Buck macro", stderr.getvalue())
        spec.unlink()
        stderr = io.StringIO()
        with mock.patch.object(sys, "argv", argv), contextlib.redirect_stderr(stderr):
            self.assertEqual(main(), 1)
        self.assertIn("launchers: ", stderr.getvalue())
