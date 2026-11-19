# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""typechecker.py — run the typechecker as a kotlin_library sub-target action.

Reads the compiling classpath from a file, unzips kotlinc-style source
archives (.src.zip, -sources.jar), checks every .kt source with the
typechecker CLI, and forwards .java sources as declaration stubs (their
top-level types resolve imports; they are never checked). Shadow mode:
always exits 0. The report records the real exit code plus some additional
information so the dashboard distinguishes not-checked from clean.
"""

import argparse
import json
import pathlib
import re
import subprocess
import sys
import time
import zipfile
from tempfile import TemporaryDirectory


def _parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--typechecker-binary", required=True, help="typechecker CLI binary")
    p.add_argument(
        "--classpath-file", required=True, help="file with the -classpath value"
    )
    p.add_argument(
        "--srcs", nargs="*", default=[], help=".kt files and source archives"
    )
    p.add_argument(
        "--generated",
        action="append",
        default=None,
        help="generated-sources dir (repeatable)",
    )
    p.add_argument("--output", required=True, help="report output path")
    p.add_argument(
        "--werror",
        action="store_true",
        help="escalate warnings to exit 1 (also forwarded to the checker)",
    )
    p.add_argument(
        "--nowarn",
        action="store_true",
        help="suppress the werror escalation (also forwarded to the checker)",
    )
    p.add_argument(
        "--warning-level",
        action="append",
        default=[],
        help="NAME:LEVEL, forwarded as -Xwarning-level (repeatable)",
    )
    p.add_argument(
        "--kotlinc-arg",
        action="append",
        default=[],
        help="kotlinc argument, forwarded to the checker in the given order (repeatable)",
    )
    p.add_argument(
        "--plugins", action="store_true", help="target uses compiler plugins"
    )
    p.add_argument(
        "--kotlin-plugin",
        action="append",
        default=[],
        help="compiler plugin jar, forwarded as -Xplugin= (repeatable)",
    )
    p.add_argument(
        "--friend-path",
        action="append",
        default=[],
        help="friend module jar, forwarded in -Xfriend-paths= (repeatable)",
    )
    p.add_argument(
        "--java-sources-skipped",
        type=int,
        default=0,
        help="count of skipped .java sources (mixed targets)",
    )
    p.add_argument(
        "--language-version",
        default=None,
        help="forwarded as -language-version (selects the CLI version profile)",
    )
    p.add_argument(
        "--jdk-base",
        default=None,
        help="hermetic java.base jar, forwarded as -jdk-home",
    )
    p.add_argument(
        "--jdk-required",
        action="store_true",
        help="fail loud (target unchecked) when --jdk-base is missing",
    )
    return p.parse_args()


# Archive suffixes mirror the bzl-side filter (zip_srcs in
# _typecheck_subtarget): keep both lists in sync.
_ARCHIVE_SUFFIXES = (".src.zip", "-sources.jar")


def _collect_sources(srcs, generated_dirs, tmp):
    out = []
    for i, s in enumerate(srcs):
        if s.endswith(_ARCHIVE_SUFFIXES):
            # Per-archive index (not len(out)): an archive yielding no .kt
            # must not collide with the next archive's destination.
            dest = f"{tmp}/{i}"
            with zipfile.ZipFile(s) as z:
                z.extractall(dest)
            out.extend(
                str(p)
                for p in sorted(pathlib.Path(dest).rglob("*"))
                if p.name.endswith((".kt", ".java"))
            )
        elif s.endswith(".kt") or s.endswith(".java"):
            # .java rides along for declaration stubs (the CLI scans
            # top-level types for import resolution; it never checks them).
            out.append(s)
    for generated in generated_dirs or []:
        out.extend(str(p) for p in sorted(pathlib.Path(generated).rglob("*.kt")))
        out.extend(str(p) for p in sorted(pathlib.Path(generated).rglob("*.java")))
    return out


_DUMP_LINE_RE = re.compile(
    r"^(?P<file>.*?):(?P<line>\d+):(?P<col>\d+):\s*(?P<severity>\w+):\s*(?P<code>.*)$"
)


def _parse_dump_line(line):
    # Wire format: RELPATH:LINE:COL: SEVERITY: FACTORY
    # Regex-anchored so a
    # colon inside the trailing factory cannot shift the line/col fields and
    # silently drop a real error line. The file group is lazy: the FIRST
    # :line:col: is the position even if the factory contains another.
    m = _DUMP_LINE_RE.match(line)
    if not m:
        return None
    return {
        "file": m.group("file"),
        "line": int(m.group("line")),
        "col": int(m.group("col")),
        "severity": m.group("severity"),
        "code": m.group("code").strip(),
    }


def _classify_exit(returncode, stderr, diagnostics, werror, nowarn):
    # Derive the report exit code from the checker result: 1 when error
    # diagnostics (or werror-escalated warnings) were recorded, 2 when the
    # checker failed without recording any (dump mode exits 0 unless it
    # crashes), else 0. -nowarn suppresses the werror escalation (kotlinc).
    # Severity vocabulary is ERROR/WARNING/INFO; compare case-insensitively
    # so a non-uppercase emitter cannot slip an error past as exit 0.
    has_error = any(d["severity"].upper() == "ERROR" for d in diagnostics)
    has_warning = any(d["severity"].upper() == "WARNING" for d in diagnostics)
    if returncode != 0 and not diagnostics:
        return 2, (stderr.strip() or "no output")[-2000:]
    if has_error or (werror and not nowarn and has_warning):
        return 1, None
    if returncode != 0:
        msg = f"checker exited {returncode} with no error diagnostics: {stderr.strip() or 'no output'}"
        return 2, msg[-2000:]
    return 0, None


def _run(args, t0):  # noqa: C901
    with open(args.classpath_file) as f:
        classpath = f.read().strip()
    with TemporaryDirectory(prefix="typechecker_") as tmp:
        sources = _collect_sources(args.srcs, args.generated, tmp)
        if not sources:
            # Nothing to check (e.g. a java-only target): report clean with
            # zero sources rather than invoking the CLI with no inputs
            # (which it rejects, producing a bogus tool_error).
            with open(args.output, "w") as f:
                json.dump(
                    {
                        "exit_code": 0,
                        "cli_exit_code": None,
                        "plugins_unchecked": args.plugins,
                        "werror": args.werror,
                        "nowarn": args.nowarn,
                        "language_version": args.language_version,
                        "java_sources_skipped": args.java_sources_skipped,
                        "sources_checked": 0,
                        "duration_s": round(time.time() - t0, 2),
                        "diagnostics": [],
                    },
                    f,
                )
            return
        # NOTE: uses --dump-diagnostics (master-compatible). When 1C-e lands,
        # switch to `check --format=json` for real exit codes and messages.
        # All CLI args go through an argfile: fleet classpaths exceed the
        # 128KB single-argument OS limit.
        #
        # Argfile contract (the CLI splits on ASCII whitespace with no quote
        # handling): fail loud on a whitespace-bearing path instead of
        # silently checking nonexistent split tokens.
        if args.jdk_required and not args.jdk_base:
            _write_crash_report(
                args.output,
                "JDK unavailable; target unchecked",
                round(time.time() - t0, 2),
                plugins=args.plugins,
                werror=args.werror,
                nowarn=args.nowarn,
                language_version=args.language_version,
                java_sources_skipped=args.java_sources_skipped,
            )
            return
        arg_paths = [classpath] if classpath else []
        if args.jdk_base:
            arg_paths.append(args.jdk_base)
        arg_paths += args.kotlin_plugin + args.friend_path
        for p in arg_paths + sources + args.kotlinc_arg:
            if any(c in " \t\n\r\x0b\x0c" for c in p):
                raise ValueError(f"argfile argument contains whitespace: {p!r}")
        argfile = tmp + "/args.txt"
        with open(argfile, "w") as f:
            f.write("--dump-diagnostics\n")
            if classpath:
                f.write("-classpath\n" + classpath + "\n")
            if args.jdk_base:
                f.write("-jdk-home\n" + args.jdk_base + "\n")
            if args.werror:
                f.write("-Werror\n")
            for level in args.warning_level:
                f.write("-Xwarning-level=" + level + "\n")
            for plugin in args.kotlin_plugin:
                f.write("-Xplugin=" + plugin + "\n")
            if args.friend_path:
                f.write("-Xfriend-paths=" + ",".join(args.friend_path) + "\n")
            if args.nowarn:
                f.write("-nowarn\n")
            if args.language_version:
                f.write("-language-version\n" + args.language_version + "\n")
            for kotlinc_arg in args.kotlinc_arg:
                f.write(kotlinc_arg + "\n")
            f.write("\n".join(sources))
        try:
            proc = subprocess.run(
                [args.typechecker_binary, "@" + argfile],
                capture_output=True,
                text=True,
                timeout=60,
            )
            cli_exit_code = proc.returncode
            diagnostics = [
                d
                for d in (_parse_dump_line(line) for line in proc.stdout.splitlines())
                if d
            ]
            exit_code, tool_error = _classify_exit(
                proc.returncode, proc.stderr, diagnostics, args.werror, args.nowarn
            )
        except subprocess.TimeoutExpired:
            cli_exit_code = None
            exit_code = 2
            diagnostics = []
            tool_error = "type checker timed out after 60s"
        report = {
            "exit_code": exit_code,
            "cli_exit_code": cli_exit_code,
            "plugins_unchecked": args.plugins,
            "werror": args.werror,
            "nowarn": args.nowarn,
            "language_version": args.language_version,
            "java_sources_skipped": args.java_sources_skipped,
            "sources_checked": len(sources),
            "duration_s": round(time.time() - t0, 2),
            "diagnostics": diagnostics,
        }
        if tool_error:
            report["tool_error"] = tool_error
        with open(args.output, "w") as f:
            json.dump(report, f)


def _write_crash_report(
    output,
    tool_error,
    duration_s,
    plugins=False,
    werror=False,
    nowarn=False,
    language_version=None,
    java_sources_skipped=0,
):
    with open(output, "w") as f:
        json.dump(
            {
                "exit_code": 2,
                "cli_exit_code": None,
                "plugins_unchecked": plugins,
                "werror": werror,
                "nowarn": nowarn,
                "language_version": language_version,
                "java_sources_skipped": java_sources_skipped,
                # Crash reports check nothing, so the count is always 0 (even
                # when sources were already collected before the crash).
                "sources_checked": 0,
                "duration_s": duration_s,
                "diagnostics": [],
                "tool_error": tool_error,
            },
            f,
        )


def _output_from_argv(argv):
    # Best-effort --output recovery when argparse itself fails: the bzl
    # always passes a literal `--output <path>` pair.
    for i, a in enumerate(argv):
        if a == "--output" and i + 1 < len(argv):
            return argv[i + 1]
    return None


def main():
    t0 = time.time()
    try:
        args = _parse_args()
    except SystemExit as e:
        # argparse failure (e.g. a value starting with `-`): no args were
        # parsed, but the action must still succeed. Recover --output from
        # argv; without it there is nowhere to write, so re-raise (a broken
        # bzl invocation must fail loud).
        output = _output_from_argv(sys.argv)
        if output is None:
            raise
        _write_crash_report(
            output,
            f"wrapper crash: argument parsing failed (exit {e.code})",
            round(time.time() - t0, 2),
        )
        return
    try:
        _run(args, t0)
    except Exception as e:
        # Shadow integrity: the action never fails. Record the crash and exit 0.
        _write_crash_report(
            args.output,
            f"wrapper crash: {repr(e)[-2000:]}",
            round(time.time() - t0, 2),
            plugins=args.plugins,
            werror=args.werror,
            nowarn=args.nowarn,
            language_version=args.language_version,
            java_sources_skipped=args.java_sources_skipped,
        )


if __name__ == "__main__":
    main()
    sys.exit(0)
