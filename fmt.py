#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Formats the Rust and Starlark in this tree exactly as Meta's internal `arc f`
does, so a pull request needs no Rust or Starlark reformatting when it is
imported.

    python3 fmt.py            # format the whole tree in place
    python3 fmt.py --check    # exit 1 and show a diff if anything would change
                              # (2 when a formatter could not run or rejected a file)
    python3 fmt.py prelude/   # only these paths

Rust goes through the nightly `rustfmt` named in `.rustfmt-version` with the
`rustfmt.toml` at the root; that is what Meta's lint runs, and it is not the
`rustfmt` of `rust-toolchain.toml`, which is why `cargo fmt` is not enough. It
is installed through `rustup` on first use. Starlark (`.bzl`, `.bxl`, `BUCK`,
...) goes through `tools/starlark_fmt`, built from this tree with `cargo`, with
the config in `tools/starlark_fmt/config.json`.

Lines carrying an `@oss-disable` or `@oss-enable` marker are rewritten by the
export from Meta's repository (`X  # @oss-disable` becomes
`# @oss-disable[end= ]: X`, `# @oss-enable: X` becomes `X # @oss-enable`),
so this tree is not what the formatters saw. Each file is formatted in its
internal form -- the rewrite undone, then redone -- so the result is exactly
what the file will look like once the change is imported and formatted there.
"""

from __future__ import annotations

import argparse
import difflib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path, PurePosixPath
from typing import NoReturn

ROOT = Path(__file__).resolve().parent

RUSTFMT_VERSION_FILE = ROOT / ".rustfmt-version"
RUSTFMT_CONFIG = ROOT / "rustfmt.toml"
STARLARK_FMT_CONFIG = ROOT / "tools" / "starlark_fmt" / "config.json"
# The `starlark_fmt` binary is the `starlark_fmt_lib` package's.
STARLARK_FMT_PACKAGE = "starlark_fmt_lib"

# Which files each formatter covers, as Meta's lint engine selects them
# (tools/arcanist/lint/fbsource-lint-engine.toml, rulesets RUSTFMT and
# BUCKFORMAT), restricted to what occurs in this tree.
STARLARK_NAMES = {"BUCK", "PACKAGE", "TARGETS", "TARGETS.fixture"}
STARLARK_SUFFIXES = {".bzl", ".bxl", ".sky"}
RUST_SUFFIXES = {".rs"}
# Files the lint engine never formats, by a token anywhere in the file.
SKIP_TOKENS = ("@" + "generated", "@" + "nolint")
LINT_IGNORE_EVERY = re.compile(r"@lint-ignore-every\b(.*)")


def is_rust(path: PurePosixPath) -> bool:
    return path.suffix in RUST_SUFFIXES and ".claude" not in path.parts


def is_starlark(path: PurePosixPath) -> bool:
    return path.name in STARLARK_NAMES or path.suffix in STARLARK_SUFFIXES


def lint_code(path: PurePosixPath) -> str:
    return "RUSTFMT" if is_rust(path) else "BUCKFORMAT"


def skipped(path: PurePosixPath, text: str) -> bool:
    if "\0" in text or any(token in text for token in SKIP_TOKENS):
        return True
    return any(
        lint_code(path) in m.group(1).split() for m in LINT_IGNORE_EVERY.finditer(text)
    )


# -- The export's comment-marker rewrite -------------------------------------
#
# A port of the `ConditionalLinesFilterMutator` that the export applies, so the
# formatting here is of the internal form of each file. `comment_start` is the
# language's, as the export's path rules assign it: `//` for Rust, `#` for
# Starlark files by extension and for `BUCK`/`TARGETS` files, nothing for
# `PACKAGE` files, whose markers the export leaves alone.

DISABLE = "@oss-disable"
ENABLE = "@oss-enable"


def comment_start(path: PurePosixPath) -> str | None:
    if is_rust(path):
        return "//"
    if path.suffix in {".bzl", ".bxl", ".sky", ".star"}:
        return "#"
    # The rule is `/BUCK` or `/TARGETS` anywhere in the path. Paths here are
    # relative to the root; internally the root is itself inside a directory.
    if "/BUCK" in f"/{path}" or "/TARGETS" in f"/{path}":
        return "#"
    return None


def comment_line(line: str, comment: str, marker: str) -> str:
    """`X <comment> <marker>` -> `<comment> <marker>[end=<ws>]: X`."""
    m = re.fullmatch(rf"(\s*)(\S.*) {re.escape(comment)} {re.escape(marker)}", line)
    if m is None:
        return line
    prefix, body = m.groups()
    content = body.rstrip()
    trailing = body[len(content) :]
    end = f"[end={trailing}]" if trailing else ""
    return f"{prefix}{comment} {marker}{end}: {content}"


def uncomment_line(line: str, comment: str, marker: str) -> str:
    """`<comment> <marker>[end=<ws>]: X` -> `X<ws> <comment> <marker>`."""
    m = re.fullmatch(
        rf"(\s*){re.escape(comment)} {re.escape(marker)}(\[end=(\s+)\])?: (.+)",
        line,
    )
    if m is None:
        return line
    prefix, _, trailing, content = m.groups()
    return f"{prefix}{content}{trailing or ''} {comment} {marker}"


def to_internal(text: str, comment: str | None) -> str:
    if comment is None:
        return text
    lines = text.split("\n")
    lines = [uncomment_line(line, comment, DISABLE) for line in lines]
    lines = [comment_line(line, comment, ENABLE) for line in lines]
    return "\n".join(lines)


def to_exported(text: str, comment: str | None) -> str:
    if comment is None:
        return text
    lines = text.split("\n")
    lines = [comment_line(line, comment, DISABLE) for line in lines]
    lines = [uncomment_line(line, comment, ENABLE) for line in lines]
    return "\n".join(lines)


# -- Formatters ---------------------------------------------------------------


def run(args: list[str], **kwargs) -> subprocess.CompletedProcess:
    return subprocess.run(args, text=True, **kwargs)


# Exit code when a formatter could not run (as opposed to 1, files that
# `--check` found unformatted or that a formatter rejected), so a caller can
# tell the two apart.
TOOL_FAILURE = 2


def tool_failure(message: str) -> NoReturn:
    print(message, file=sys.stderr)
    sys.exit(TOOL_FAILURE)


def find_rustfmt(explicit: str | None) -> list[str]:
    if explicit:
        if not shutil.which(explicit):
            tool_failure(f"--rustfmt={explicit}: not an executable")
        return [explicit]
    if not RUSTFMT_VERSION_FILE.exists():
        tool_failure(
            f"{RUSTFMT_VERSION_FILE.name} is missing; run from a full checkout"
        )
    toolchain = RUSTFMT_VERSION_FILE.read_text().strip()
    rustfmt = ["rustup", "run", toolchain, "rustfmt"]
    try:
        installed = run(rustfmt + ["--version"], capture_output=True).returncode == 0
    except FileNotFoundError:
        tool_failure(
            "rustup is not on PATH (https://rustup.rs), and no --rustfmt given"
        )
    if not installed:
        print(f"Installing rustfmt {toolchain} through rustup (once)", flush=True)
        if (
            run(
                [
                    "rustup",
                    "toolchain",
                    "install",
                    toolchain,
                    "--profile=minimal",
                    "--component=rustfmt",
                    "--no-self-update",
                ]
            ).returncode
            != 0
        ):
            tool_failure(f"installing rustfmt {toolchain} failed")
    return rustfmt


def find_starlark_fmt(explicit: str | None) -> str:
    if explicit:
        if not shutil.which(explicit):
            tool_failure(f"--starlark-fmt={explicit}: not an executable")
        return explicit
    print("Building starlark_fmt with cargo", flush=True)
    result = run(
        [
            "cargo",
            "build",
            f"--package={STARLARK_FMT_PACKAGE}",
            "--bin=starlark_fmt",
            "--message-format=json-render-diagnostics",
        ],
        cwd=ROOT,
        stdout=subprocess.PIPE,
    )
    if result.returncode != 0:
        tool_failure("building starlark_fmt failed")
    for line in result.stdout.splitlines():
        message = json.loads(line)
        if message.get("reason") == "compiler-artifact" and message.get("executable"):
            return message["executable"]
    tool_failure("cargo did not report the starlark_fmt executable")


def chunks(items: list, size: int) -> list[list]:
    return [items[i : i + size] for i in range(0, len(items), size)]


def rejected_files(
    command: list[str], files: list[Path], tool: str, cwd: Path | None = None
) -> list[Path]:
    """
    After `command` failed on `files`, the ones it fails on by itself. A
    formatter that fails on every one of several files did not run at all,
    which is a tool failure; one that fails on some rejected those (a syntax
    error, say).
    """
    rejected = [
        f
        for f in files
        if run(command + [str(f)], cwd=cwd, capture_output=True).returncode != 0
    ]
    if len(files) > 1 and len(rejected) == len(files):
        tool_failure(f"{tool} failed on every file")
    return rejected


def format_rust(rustfmt: list[str], mirror: Path, files: list[Path]) -> list[Path]:
    """
    Formats `files` (under `mirror`) in place, each as a standalone module;
    returns the files rustfmt rejected.
    """
    if not files:
        return []
    command = rustfmt + [
        f"--config-path={RUSTFMT_CONFIG}",
        "--unstable-features",
        "--skip-children",
        "--quiet",
    ]
    # Command lines are bounded (Windows: 32 KiB).
    batches = chunks(files, 50)
    with ThreadPoolExecutor(os.cpu_count() or 1) as pool:
        results = pool.map(
            lambda chunk: run(command + [str(f) for f in chunk], capture_output=True),
            batches,
        )
    failed = []
    for chunk, result in zip(batches, results):
        if result.returncode != 0:
            failed += chunk
            # The files are under the mirror; name them as in the tree.
            sys.stderr.write(result.stderr.replace(f"{mirror}{os.sep}", ""))
    return rejected_files(command, failed, "rustfmt") if failed else []


def format_starlark(starlark_fmt: str, mirror: Path, files: list[Path]) -> list[Path]:
    """
    Formats `files` (under `mirror`) in place, paths selecting config
    overrides; returns the files starlark_fmt rejected.
    """
    if not files:
        return []
    command = [starlark_fmt, f"--config={STARLARK_FMT_CONFIG}", "fmt"]
    argfile = mirror / "starlark_fmt_files.txt"
    argfile.write_text("".join(f"{f.relative_to(mirror)}\n" for f in files))
    if run(command + [f"@{argfile}"], cwd=mirror).returncode == 0:
        return []
    relative = [f.relative_to(mirror) for f in files]
    return [
        mirror / f
        for f in rejected_files(command, relative, "starlark_fmt", cwd=mirror)
    ]


# -- Driver -------------------------------------------------------------------


def tracked_files(paths: list[str]) -> list[PurePosixPath]:
    """Tracked and untracked files, minus ignored ones, as `arc f` sees them."""
    result = run(
        [
            "git",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            *paths,
        ],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        check=True,
    )
    return [PurePosixPath(p) for p in result.stdout.split("\0") if p]


def collect(paths: list[str]) -> dict[PurePosixPath, str]:
    """The files to format, with their current contents."""
    files = {}
    for path in tracked_files(paths):
        if not (is_rust(path) or is_starlark(path)):
            continue
        file = ROOT / path
        if file.is_symlink() or not file.is_file():
            continue
        try:
            text = file.read_bytes().decode("utf-8")
        except UnicodeDecodeError:
            continue
        if not skipped(path, text):
            files[path] = text
    return files


def format_all(
    originals: dict[PurePosixPath, str], rustfmt: str | None, starlark_fmt: str | None
) -> tuple[dict[PurePosixPath, str], list[PurePosixPath]]:
    """
    The formatted contents of `originals`, for the files that change, and
    the files a formatter rejected. Each file is formatted in its internal
    form, under a mirror of the tree, and rewritten back to the exported form.
    """
    rust_files = [p for p in originals if is_rust(p)]
    starlark_files = [p for p in originals if not is_rust(p)]
    rustfmt_command = find_rustfmt(rustfmt) if rust_files else []
    starlark_fmt_path = find_starlark_fmt(starlark_fmt) if starlark_files else ""

    with tempfile.TemporaryDirectory(prefix="buck2-fmt-") as tmp:
        mirror = Path(tmp)
        for path, text in originals.items():
            target = mirror / path
            target.parent.mkdir(parents=True, exist_ok=True)
            internal = to_internal(text.replace("\r\n", "\n"), comment_start(path))
            target.write_bytes(internal.encode("utf-8"))
        rejected = format_rust(
            rustfmt_command, mirror, [mirror / p for p in rust_files]
        ) + format_starlark(
            starlark_fmt_path, mirror, [mirror / p for p in starlark_files]
        )
        rejected_paths = [
            PurePosixPath(f.relative_to(mirror).as_posix()) for f in rejected
        ]

        changed = {}
        for path, text in originals.items():
            if path in rejected_paths:
                continue
            formatted = (mirror / path).read_bytes().decode("utf-8")
            formatted = to_exported(formatted, comment_start(path))
            if "\r\n" in text:
                formatted = formatted.replace("\n", "\r\n")
            if formatted != text:
                changed[path] = formatted
    return changed, rejected_paths


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--check",
        action="store_true",
        help="Report files that would change instead of rewriting them",
    )
    parser.add_argument(
        "--rustfmt",
        help="rustfmt executable to use instead of the pinned nightly through rustup",
    )
    parser.add_argument(
        "--starlark-fmt",
        help="starlark_fmt executable to use instead of building it with cargo",
    )
    parser.add_argument(
        "--changed-list",
        type=Path,
        help="Also write the paths of the files that were (with --check: would be) "
        "reformatted here, one per line; a file a formatter rejected is followed "
        "by a tab and `rejected`",
    )
    parser.add_argument(
        "paths", nargs="*", help="Files or directories to format (default: all)"
    )
    args = parser.parse_args()

    for required in [RUSTFMT_CONFIG, STARLARK_FMT_CONFIG]:
        if not required.exists():
            tool_failure(
                f"{required.relative_to(ROOT)} is missing; run from a full checkout"
            )

    originals = collect(args.paths)
    changed, rejected = format_all(originals, args.rustfmt, args.starlark_fmt)
    if args.changed_list is not None:
        args.changed_list.write_text(
            "".join(f"{p}\n" for p in changed)
            + "".join(f"{p}\trejected\n" for p in rejected)
        )
    if rejected:
        print(f"{len(rejected)} file(s) rejected by a formatter; see above:")
        print("".join(f"  {p}\n" for p in rejected), end="")
    if not changed:
        if not rejected:
            print(f"{len(originals)} files already formatted")
        return 1 if rejected else 0

    if not args.check:
        for path, formatted in changed.items():
            (ROOT / path).write_bytes(formatted.encode("utf-8"))
        print(f"Reformatted {len(changed)} file(s)")
        return 1 if rejected else 0

    for path, formatted in changed.items():
        sys.stdout.writelines(
            difflib.unified_diff(
                originals[path].splitlines(keepends=True),
                formatted.splitlines(keepends=True),
                f"a/{path}",
                f"b/{path}",
            )
        )
    print(
        f"\n{len(changed)} file(s) are not formatted; "
        f"run `python3 {Path(__file__).name}` to fix them:"
    )
    print("".join(f"  {p}\n" for p in changed), end="")
    return 1


if __name__ == "__main__":
    sys.exit(main())
