# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Build package trees for the prelude's Node.js rules.

package_tree.py assemble-package (--emit DIR [--package-subdir PATH] | --prebuilt DIR) --entries FILE [--package-json FILE] --out DIR
package_tree.py launchers --spec FILE --out DIR
package_tree.py archive --src DIR --format FORMAT --out FILE [--root ROOT] [--executables FILE]
    [--sha256 FILE]
package_tree.py compress --src FILE --out OUT [--sha256 FILE] --zstd ZSTD...
package_tree.py manifest --tree DIR --out FILE
package_tree.py strip-types --src DIR --out DIR
package_tree.py build --dir DIR --output-path PATH --out DIR [--root-prefix PREFIX] [--env-key KEY]... --command COMMAND...
package_tree.py validate-package --package DIR --name NAME [--dir-name NAME] [--main PATH] [--bin NAME:PATH]... [--type TYPE] [--version VERSION]
    [--module DIR [--module-src-root PATH]] --out FILE

The `--entries` file of `assemble-package` is a JSON list of
`[path, src, name]` entries: `src` is copied to the package-relative `path`,
and `name` describes the entry in error messages.

`build` runs `COMMAND` in a writable copy of a staged directory and copies the
directory it writes at `PATH` to `--out`.

`validate-package` writes a Buck2 validation result: a failure lists every
problem with the package's `package.json`.
"""

from __future__ import annotations

import argparse
import contextlib
import gzip
import hashlib
import itertools
import json
import os
import posixpath
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from collections.abc import Collection, Iterator, Sequence
from pathlib import Path
from typing import BinaryIO, NamedTuple


def assemble_package(
    base: Path,
    package_subdir: str,
    entries: list[tuple[str, Path, str]],
    package_json: Path | None,
    out: Path,
    origin: str = "emitted",
) -> list[str]:
    """Lay out `base` at `package_subdir` inside `out`, copy each `(path, src, name)` entry to `path`, then `package_json` to `package.json`.

    Entries merge into base directories and symlinks are copied as their
    targets. Returns one message per collision with an `origin` path and per
    failed copy, a symlink cycle in an entry included. Raises `ValueError` on a
    symlink cycle in `base` and `OSError` if `emit` cannot be copied.
    """
    _copy_resolved(
        base, out.joinpath(*package_subdir.split("/")) if package_subdir else out, set()
    )
    errors = []
    for path, src, name in entries:
        try:
            blocked = _make_parent_dirs(out, path)
        except OSError as e:
            errors.append(f"{name}: cannot copy '{path}': {e.strerror or e}")
            continue
        if blocked is not None:
            errors.append(f"{name} collides with the {origin} file '{blocked}'")
        else:
            _add_entry(src, out / path, path, name, origin, errors, set())
    if package_json is not None:
        manifest = out / "package.json"
        if manifest.is_dir():
            errors.append(
                f"`package_json` collides with the {origin} directory 'package.json'"
            )
        else:
            try:
                if os.path.lexists(manifest):
                    # The base's copy keeps its source's mode, which may be read-only.
                    os.chmod(manifest, stat.S_IREAD | stat.S_IWRITE)
                shutil.copy(package_json, manifest)
            except OSError as e:
                errors.append(
                    f"`package_json`: cannot copy 'package.json': {e.strerror or e}"
                )
    return errors


def _make_parent_dirs(out: Path, path: str) -> str | None:
    """Create the directories above `path` inside `out`, or return the first of them that is a file."""
    parent = out
    segments = path.split("/")[:-1]
    for i, segment in enumerate(segments):
        parent = parent / segment
        if not parent.is_dir():
            if os.path.lexists(parent):
                return "/".join(segments[: i + 1])
            parent.mkdir()
    return None


def _add_entry(
    src: Path,
    dst: Path,
    path: str,
    name: str,
    origin: str,
    errors: list[str],
    ancestors: set[tuple[int, int]],
) -> None:
    if not src.is_dir():
        if os.path.lexists(dst):
            kind = "directory" if dst.is_dir() else "file"
            errors.append(f"{name} collides with the {origin} {kind} '{path}'")
            return
        try:
            shutil.copy(src, dst)
        except OSError as e:
            errors.append(f"{name}: cannot copy '{path}': {e.strerror or e}")
        return
    if os.path.lexists(dst) and not dst.is_dir():
        errors.append(f"{name} collides with the {origin} file '{path}'")
        return
    try:
        key = _enter_dir(src, ancestors)
    except ValueError as e:
        errors.append(f"{name}: cannot copy '{path}': {e}")
        return
    try:
        dst.mkdir(exist_ok=True)
        with os.scandir(src) as entries:
            children = sorted(entries, key=lambda e: e.name)
    except OSError as e:
        errors.append(f"{name}: cannot copy '{path}': {e.strerror or e}")
        ancestors.remove(key)
        return
    for entry in children:
        _add_entry(
            Path(entry.path),
            dst / entry.name,
            f"{path}/{entry.name}",
            name,
            origin,
            errors,
            ancestors,
        )
    ancestors.remove(key)


def _enter_dir(path: Path, ancestors: set[tuple[int, int]]) -> tuple[int, int]:
    st = os.stat(path)
    key = (st.st_dev, st.st_ino)
    if key in ancestors:
        raise ValueError(
            f"symlink cycle: {path} resolves to a directory that contains it"
        )
    ancestors.add(key)
    return key


LAUNCHERS_HELP = """\
Write a POSIX sh launcher DIR/bin/NAME and a Windows launcher DIR/bin/NAME.cmd for each entry of
an app whose root is the parent of bin.

FILE is a JSON object:
  launchers  [{"name": NAME, "entry": PATH}], PATH being relative to the root, `/`-separated
  runtime    {"kind": "native" | "pointer" | "host",
              "posix_exe": PATH, "windows_exe": PATH,  (required unless kind is "host")
              "node_args": [str], "env": {str: str}}
  node_args  [str]
  args       [str]
  env        {str: str}

A launcher runs NODE, then the runtime's node_args, node_args, the entry, args and the
launcher's own arguments, with the runtime's env and then env set; an empty value unsets the
variable, and no two names may differ only in case. NODE is $NODEJS_LAUNCHER_NODE when set and non-empty, else ROOT/posix_exe (sh) or ROOT\\windows_exe
(cmd), or `node` from PATH for a host runtime, whose cmd launcher then drops .JS from PATHEXT.
`{app_root}` expands to ROOT when the launcher runs; values are otherwise literal.
"""

_APP_ROOT = "{app_root}"
_HOST_NODE = "node"
_MACRO = re.compile(r"\$\(@?[A-Za-z0-9_-]+[\s)]")
_ENV_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_EXE = re.compile(r"[\w+-][\w.+-]*(/[\w+-][\w.+-]*)*", re.ASCII)
_SH_SAFE = re.compile(r"[\w@%+=:,./-]+", re.ASCII)
_SH_ESCAPED = re.compile(r'[\\"$`]')
_WINDOWS_DEVICE = re.compile(
    r"(CON|PRN|AUX|NUL|COM[0-9]|LPT[0-9])(\..*)?", re.IGNORECASE
)
# Variables the launchers set for themselves, and ERRORLEVEL: a variable of that name would hide
# node's exit code from `exit /b %ERRORLEVEL%`.
_RESERVED_ENV = frozenset(["__ROOT", "__NODE", "__CARET", "ERRORLEVEL"])
# The default PATHEXT.
_WINDOWS_EXE_EXTENSIONS = frozenset(
    [
        ".com",
        ".exe",
        ".bat",
        ".cmd",
        ".vbs",
        ".vbe",
        ".js",
        ".jse",
        ".wsf",
        ".wsh",
        ".msc",
    ]
)

# A sh launcher resolves its own path through symlinks without `readlink -f`, which older macOS
# and BSDs lack. The variables it unsets would make `ls` quote or color names, or `cd` search.
# bash 3.2, macOS's sh, ends a `$(` at the `)` that balances it, so case patterns open with `(`.
_SH_ROOT = """\
__ROOT=$(
  unset CDPATH CLICOLOR_FORCE QUOTING_STYLE
  self=$0
  while
    dir=${self%"${self##*/}"}
    [ -h "$self" ]
  do
    link=$(ls -ld -- "$self")
    link=${link#*"$self -> "}
    case $link in
      (/*) self=$link ;;
      (*) self=$dir$link ;;
    esac
  done
  cd -P -- "${dir:-./}.." && pwd -P
) || exit"""

# Arguments with any of these are quoted: whitespace and `,;=` separate arguments for the C
# runtime or for batch files, and cmd.exe acts on the rest.
_CMD_SPECIAL = frozenset(' \t&|<>^()%!",;=')
# `call` expands `%` twice and doubles each caret within quotes, so a caret comes from a variable
# that only the second expansion reads, as does the app root.
_CMD_CALL_ESCAPES = {"%": "%%%%", "^": "%%__CARET%%"}
# FOR variables that are not also `%~` modifiers.
_CMD_FOR_VARIABLES = "RBCEGHIJKLMOQUVWYbceghijklmoquvwy"


def _check_object(value: object, keys: set[str], what: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise ValueError(f"{what} must be an object")
    unknown = sorted(set(value) - keys)
    if unknown:
        raise ValueError(f"{what} has unknown keys {unknown}")
    return value


def _check_value(value: object, what: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{what} must be a string, not {value!r}")
    bad = next((c for c in value if c != "\t" and not " " <= c <= "~"), None)
    if bad is not None:
        # cmd.exe decodes batch files in the console code page.
        raise ValueError(
            f"{what} {value!r} has {bad!r}; launchers hold printable ASCII only"
        )
    if _MACRO.search(value):
        raise ValueError(
            f"{what} {value!r} has a Buck macro, which a launcher cannot expand; "
            f"refer to files in the app with `{_APP_ROOT}`"
        )
    return value


def _is_portable_name(name: str) -> bool:
    return not (
        name in ("", ".", "..")
        or name[-1] in ". "
        or any(c in '\t<>:"\\|?*' for c in name)
        or _WINDOWS_DEVICE.fullmatch(name)
    )


def _check_path(value: object, what: str) -> str:
    path = _check_value(value, what)
    if not all(map(_is_portable_name, path.split("/"))):
        raise ValueError(f"{what} {path!r} is not a portable relative path")
    return path


def _strings(obj: dict[str, object], key: str, what: str) -> list[str]:
    values = obj.get(key)
    if values is None:
        return []
    if not isinstance(values, list):
        raise ValueError(f"{what} must be a list of strings")
    return [_check_value(v, f"{what}[{i}]") for i, v in enumerate(values)]


def _env(obj: dict[str, object], what: str) -> dict[str, str]:
    env = obj.get("env")
    if env is None:
        return {}
    if not isinstance(env, dict):
        raise ValueError(f"{what} must be an object")
    checked = {}
    for name, value in env.items():
        if not _ENV_NAME.fullmatch(name) or name.upper() in _RESERVED_ENV:
            raise ValueError(f"{what} cannot set {name!r}")
        checked[name] = _check_value(value, f"{what}[{name!r}]")
    return checked


def _runtime_nodes(runtime: dict[str, object]) -> tuple[str, str]:
    """The node that sh and cmd launchers run when `NODEJS_LAUNCHER_NODE` is unset or empty."""
    kind = runtime.get("kind")
    if kind == "host":
        if (
            runtime.get("posix_exe") is not None
            or runtime.get("windows_exe") is not None
        ):
            raise ValueError("a host runtime has no posix_exe or windows_exe")
        return _HOST_NODE, _HOST_NODE
    if kind not in ("native", "pointer"):
        raise ValueError(f"runtime.kind must be native, pointer or host, not {kind!r}")
    exes = []
    for key in ("posix_exe", "windows_exe"):
        exe = _check_path(runtime.get(key), f"runtime.{key}")
        if not _EXE.fullmatch(exe):
            raise ValueError(
                f"runtime.{key} {exe!r} has characters launchers do not quote"
            )
        exes.append(exe)
    return "$__ROOT/" + exes[0], "%__ROOT%\\" + exes[1].replace("/", "\\")


def _sh_word(parts: list[str]) -> str:
    """Quotes `parts`, joined by the app root, as one sh word."""
    if len(parts) == 1 and _SH_SAFE.fullmatch(parts[0]):
        return parts[0]
    # Double quotes, since shellcheck flags `$` or a final backslash within single quotes.
    return (
        '"'
        + "${__ROOT}".join(_SH_ESCAPED.sub(r"\\\g<0>", part) for part in parts)
        + '"'
    )


def _sh_launcher(
    node: str, node_args: list[str], entry: str, args: list[str], env: dict[str, str]
) -> str:
    lines = ["#!/bin/sh", _SH_ROOT, f"__NODE=${{NODEJS_LAUNCHER_NODE:-{node}}}"]
    for name, value in env.items():
        parts = value.split(_APP_ROOT)
        lines.append(f"export {name}={_sh_word(parts)}" if value else f"unset {name}")
    words = [_sh_word(arg.split(_APP_ROOT)) for arg in node_args]
    words.append(_sh_word(["", "/" + entry]))
    words += [_sh_word(arg.split(_APP_ROOT)) for arg in args]
    lines.append(" ".join(['exec "$__NODE"', *words, '"$@"']))
    return "\n".join(lines) + "\n"


def _cmd_call_arg(parts: list[str]) -> str:
    """Quotes `parts`, joined by the app root, as one argument on a launcher's `call` line.

    The program gets the argument if it parses its command line like the Microsoft C runtime,
    or if it is a batch file that passes `%*` on to such a program, since the result has cmd.exe
    metacharacters only within quotes.
    """
    # `call` prints its help instead of running the program if an unquoted argument has `/?`.
    if (
        len(parts) == 1
        and parts[0]
        and not _CMD_SPECIAL.intersection(parts[0])
        and "/?" not in parts[0]
    ):
        return parts[0]
    out = ['"']
    for i, part in enumerate(parts):
        if i:
            out.append("%%__ROOT%%")
        backslashes = 0
        for c in part:
            if c == "\\":
                backslashes += 1
            else:
                if c == '"':
                    # `\"` would end the quotes for cmd.exe, so `"` is written `""`.
                    out.append("\\" * backslashes + '"')
                backslashes = 0
            out.append(_CMD_CALL_ESCAPES.get(c, c))
        if i == len(parts) - 1:
            out.append("\\" * backslashes + '"')
    return "".join(out)


def _cmd_set(name: str, value: str) -> str:
    """A batch line that sets `name` to `value`, or unsets it when `value` is empty.

    `set "NAME=VALUE"` keeps the quotes within VALUE, and after an odd number of them the line
    is unquoted, so metacharacters there get carets. The app root comes from a FOR variable,
    which cmd.exe substitutes after parsing the line.
    """
    parts = value.split(_APP_ROOT)
    var = None
    if len(parts) > 1:
        var = next((v for v in _CMD_FOR_VARIABLES if v not in value), None)
        if var is None:
            raise ValueError(
                f"env value {value!r} leaves no FOR variable for `{_APP_ROOT}`"
            )
    out = []
    quoted = True
    for i, part in enumerate(parts):
        if i:
            out.append(f"%%~{var}")
        for c in part:
            if c == '"':
                quoted = not quoted
            elif c == "%":
                c = "%%"
            elif not quoted and c in "^&|<>()":
                c = "^" + c
            out.append(c)
    line = f'set "{name}={"".join(out)}"'
    return f'for %%{var} in ("%__ROOT%") do {line}' if var else line


def _cmd_launcher(
    node: str, node_args: list[str], entry: str, args: list[str], env: dict[str, str]
) -> str:
    call_args = [arg.split(_APP_ROOT) for arg in node_args]
    call_args.append(["", "\\" + entry.replace("/", "\\")])
    call_args += [arg.split(_APP_ROOT) for arg in args]
    lines = [
        "@echo off",
        "setlocal EnableExtensions DisableDelayedExpansion",
        # `%~dp0` names the wrong directory at the top level of a script run as `"NAME"` from
        # PATH, but not in a subroutine.
        "call :root",
        'set "__NODE=%NODEJS_LAUNCHER_NODE%"',
        f'if not defined __NODE set "__NODE={node}"',
    ]
    if any("^" in part for parts in call_args for part in parts):
        lines.append('set "__CARET=^"')
    lines += [_cmd_set(name, value) for name, value in env.items()]
    if node == _HOST_NODE:
        # cmd.exe looks for `node` in the current directory first, trying each PATHEXT extension,
        # so it would hand a `node.js` there to Windows Script Host.
        lines += [
            'if defined PATHEXT set "PATHEXT=;%PATHEXT:;=;;%;"',
            'if defined PATHEXT set "PATHEXT=%PATHEXT:;.JS;=%"',
            'if defined PATHEXT set "PATHEXT=%PATHEXT:;;=;%"',
            'if defined PATHEXT set "PATHEXT=%PATHEXT:~1,-1%"',
        ]
    lines += [
        # Node inherits `__ROOT`, `__NODE` and `__CARET`: `call` expands them as it runs.
        " ".join(['call "%%__NODE%%"', *map(_cmd_call_arg, call_args), "%%*"]),
        "exit /b %ERRORLEVEL%",
        ":root",
        'set "__ROOT=%~dp0.."',
        "exit /b",
    ]
    return "\r\n".join(lines) + "\r\n"


def _check_launcher(i: int, launcher: object) -> tuple[str, str]:
    fields = _check_object(launcher, {"name", "entry"}, f"launchers[{i}]")
    name = _check_path(fields.get("name"), f"launchers[{i}].name")
    if "/" in name:
        raise ValueError(f"launchers[{i}].name {name!r} has a `/`")
    # Also `node.exe` and the like: cmd.exe finds their sh launchers for `node`.
    if name.lower().split(".")[0] == _HOST_NODE:
        raise ValueError(
            f"launchers[{i}].name {name!r} would shadow node with bin on PATH"
        )
    # cmd.exe would run the sh launcher itself for NAME.
    _, dot, extension = name.lower().rpartition(".")
    if dot and "." + extension in _WINDOWS_EXE_EXTENSIONS:
        raise ValueError(
            f"launchers[{i}].name {name!r} ends in a Windows executable extension"
        )
    return name, _check_path(fields.get("entry"), f"launchers[{i}].entry")


def write_launchers(spec: object, out: Path) -> None:
    """Write the launchers that `spec` describes into `out`; see `LAUNCHERS_HELP`."""
    checked = _check_object(
        spec, {"launchers", "runtime", "node_args", "args", "env"}, "spec"
    )
    runtime = _check_object(
        checked.get("runtime"),
        {"kind", "posix_exe", "windows_exe", "node_args", "env"},
        "runtime",
    )
    posix_node, windows_node = _runtime_nodes(runtime)
    node_args = _strings(runtime, "node_args", "runtime.node_args")
    node_args += _strings(checked, "node_args", "node_args")
    args = _strings(checked, "args", "args")
    env = {**_env(runtime, "runtime.env"), **_env(checked, "env")}
    names = {}
    for name in env:
        other = names.setdefault(name.upper(), name)
        if other != name:
            raise ValueError(f"env sets both {other!r} and {name!r}, one cmd variable")

    launchers = checked.get("launchers")
    if not isinstance(launchers, list) or not launchers:
        raise ValueError("launchers must be a non-empty list")
    files = {}
    for i, launcher in enumerate(launchers):
        name, entry = _check_launcher(i, launcher)
        sh = _sh_launcher(posix_node, node_args, entry, args, env)
        cmd = _cmd_launcher(windows_node, node_args, entry, args, env)
        for file, text, mode in ((name, sh, 0o755), (name + ".cmd", cmd, 0o644)):
            if file.lower() in files:
                raise ValueError(f"launchers write bin/{file} twice")
            files[file.lower()] = (file, text, mode)

    bin_dir = out / "bin"
    bin_dir.mkdir(parents=True, exist_ok=True)
    for file, text, mode in files.values():
        path = bin_dir / file
        path.write_bytes(text.encode("ascii"))
        path.chmod(mode)


ARCHIVE_FORMATS = ("zip", "tar", "tar.gz")

ARCHIVE_HELP = """\
Write the contents of DIR to FILE as a FORMAT archive, under ROOT in the archive (by default, at
its top), and FILE's SHA-256 in hex, with a newline, to the --sha256 file.

Entries are in depth-first order, sorted by name within each directory. Directories are 0755, tar
symlinks 0777, and files 0755 if their owner can execute them or the --executables file lists them,
else 0644. Windows has no execute bits, so there only listed files are 0755. Each nonblank line of
the --executables file, less surrounding whitespace, is the `/`-separated path of a file in DIR that
neither is nor goes through a symlink.

zip entries are Unix entries dated 1985-02-01 00:00:00, with files deflated at level 9. tar entries
are PAX, dated 1985-10-26T08:15:00Z, with uid and gid 0 and no owner names. tar.gz is gzip at level
9 with no name or mtime. So the archive depends only on ROOT, the --executables file, the names,
contents, execute bits and symlinks in DIR, and the Python that writes it, including, for zip and
tar.gz, its zlib. If writing fails, FILE and the --sha256 file are removed.

A symlink must be relative, with `..` only at the start of its target, and resolve to a file or
directory in DIR without passing outside it. tar stores symlinks; zip stores what they point to,
and fails on a symlink to a directory that contains it.
"""

COMPRESS_HELP = """\
Compress FILE to OUT by running `ZSTD... -o OUT FILE`, and write OUT's SHA-256 in hex, with a
newline, to the --sha256 file. ZSTD is the rest of the command line, so --zstd comes last. If ZSTD
or writing the --sha256 file fails, OUT and the --sha256 file are removed.
"""

# 1985-10-26T08:15:00Z, the mtime npm gives every entry of a package tarball.
_TAR_MTIME = 499162500
# The DOS time that Buck's Java zip tool gives every entry.
_ZIP_DATE_TIME = (1985, 2, 1, 0, 0, 0)
_DEFLATE_LEVEL = 9
_DIR_MODE = stat.S_IFDIR | 0o755


class _Entry(NamedTuple):
    name: str
    mode: int
    # The file to copy, the target of a symlink, or "" for a directory.
    source: str


def _check_symlink(
    path: str, name: str, location: str, real_root: str
) -> tuple[str, str]:
    """Checks the symlink `path`, whose path in `real_root` without symlinks is `location`.

    Returns its target and the path in `real_root` without symlinks of what it resolves to.
    """
    target = os.readlink(path).replace(os.sep, "/")
    if target.startswith("/") or os.path.splitdrive(target)[0]:
        raise ValueError(f"symlink {name} -> {target} is absolute")
    try:
        os.stat(path)
    except OSError as e:
        raise ValueError(
            f"symlink {name} -> {target} does not resolve: {e.strerror}"
        ) from e
    real = os.path.realpath(path)
    inside = os.path.join(real_root, "")
    if real != real_root and not real.startswith(inside):
        raise ValueError(f"symlink {name} -> {target} resolves outside the directory")
    # Extracted, the target resolves from the symlink's place under another root, so its path
    # must stay in the directory: its leading `..`s may not climb above it, and no `..` may
    # follow a name, since after a symlink `..` climbs from where the symlink points.
    parts = [part for part in target.split("/") if part not in ("", ".")]
    names = list(itertools.dropwhile(lambda part: part == "..", parts))
    if ".." in names:
        raise ValueError(f"symlink {name} -> {target} has `..` after a name")
    if len(parts) - len(names) > location.count("/"):
        raise ValueError(f"symlink {name} -> {target} passes outside the directory")
    return target, real[len(inside) :].replace(os.sep, "/")


def _archive_entries(
    src: Path, root: str, follow_symlinks: bool, executables: Collection[str]
) -> list[_Entry]:
    if root and not (
        root.isprintable() and all(map(_is_portable_name, root.split("/")))
    ):
        raise ValueError(f"root {root!r} is not a portable relative path")
    parts = root.split("/") if root else []
    entries = [
        _Entry("/".join(parts[: i + 1]), _DIR_MODE, "") for i in range(len(parts))
    ]
    prefix = "".join(part + "/" for part in parts)
    real_root = os.path.realpath(src)
    listed = frozenset(executables)
    files = set()

    # `locations` are the paths in `src`, without symlinks, of `directory` and of the directories
    # the walk went through to reach it.
    def walk(directory: str, rel: str, locations: tuple[str, ...]) -> None:
        for entry in sorted(os.scandir(directory), key=lambda e: e.name):
            name = rel + entry.name
            location = posixpath.join(locations[-1], entry.name)
            st = entry.stat(follow_symlinks=False)
            if stat.S_ISLNK(st.st_mode):
                target, location = _check_symlink(entry.path, name, location, real_root)
                if not follow_symlinks:
                    entries.append(_Entry(prefix + name, stat.S_IFLNK | 0o777, target))
                    continue
                if location in locations:
                    raise ValueError(
                        f"symlink {name} -> {target} points to a directory that contains it"
                    )
                st = os.stat(entry.path)
            if stat.S_ISDIR(st.st_mode):
                entries.append(_Entry(prefix + name, _DIR_MODE, ""))
                walk(entry.path, name + "/", (*locations, location))
            elif stat.S_ISREG(st.st_mode):
                files.add(location)
                # Windows has no execute bits, though `os.stat` makes some up from file names.
                executable = location in listed or (
                    sys.platform != "win32" and st.st_mode & stat.S_IXUSR
                )
                mode = 0o755 if executable else 0o644
                entries.append(_Entry(prefix + name, stat.S_IFREG | mode, entry.path))
            else:
                raise ValueError(f"{name} is not a file, directory or symlink")

    walk(os.fspath(src), "", ("",))
    unknown = sorted(listed - files)
    if unknown:
        raise ValueError(
            f"executable {unknown[0]} names no file in the directory without following symlinks"
        )
    return entries


def _write_zip(entries: list[_Entry], out: Path) -> None:
    with zipfile.ZipFile(out, "w") as archive:
        for entry in entries:
            is_dir = stat.S_ISDIR(entry.mode)
            info = zipfile.ZipInfo(
                entry.name + "/" if is_dir else entry.name, _ZIP_DATE_TIME
            )
            # Unix, so that unzip and the like apply the modes in external_attr.
            info.create_system = 3
            # 0x10 is the MS-DOS directory attribute.
            info.external_attr = entry.mode << 16 | (0x10 if is_dir else 0)
            if is_dir:
                archive.writestr(info, b"")
            else:
                # `writestr` needs the whole file, but before Python 3.13 it is the only public
                # way to give a `ZipInfo` a compression level.
                with open(entry.source, "rb") as f:
                    archive.writestr(
                        info,
                        f.read(),
                        compress_type=zipfile.ZIP_DEFLATED,
                        compresslevel=_DEFLATE_LEVEL,
                    )


def _write_tar(entries: list[_Entry], fileobj: BinaryIO) -> None:
    with tarfile.open(
        fileobj=fileobj, mode="w", format=tarfile.PAX_FORMAT, encoding="utf-8"
    ) as archive:
        for entry in entries:
            # uid and gid default to 0, and uname and gname to "".
            info = tarfile.TarInfo(entry.name)
            info.mode = stat.S_IMODE(entry.mode)
            info.mtime = _TAR_MTIME
            if stat.S_ISDIR(entry.mode):
                info.type = tarfile.DIRTYPE
                archive.addfile(info)
            elif stat.S_ISLNK(entry.mode):
                info.type = tarfile.SYMTYPE
                info.linkname = entry.source
                archive.addfile(info)
            else:
                with open(entry.source, "rb") as f:
                    info.size = os.fstat(f.fileno()).st_size
                    archive.addfile(info, f)


def _write_sha256(path: Path, out: Path) -> None:
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    out.write_bytes(digest.hexdigest().encode("ascii") + b"\n")


@contextlib.contextmanager
def _removed_on_failure(*paths: Path | None) -> Iterator[None]:
    try:
        yield
    except BaseException:
        for path in paths:
            if path is not None:
                path.unlink(missing_ok=True)
        raise


def write_archive(
    src: Path,
    archive_format: str,
    out: Path,
    root: str = "",
    executables: Collection[str] = (),
    sha256: Path | None = None,
) -> None:
    """Write `src` to `out` as an `archive_format` archive; see `ARCHIVE_HELP`."""
    if archive_format not in ARCHIVE_FORMATS:
        raise ValueError(f"unknown archive format {archive_format!r}")
    entries = _archive_entries(
        src, root, follow_symlinks=archive_format == "zip", executables=executables
    )
    with _removed_on_failure(out, sha256):
        if archive_format == "zip":
            _write_zip(entries, out)
        else:
            with open(out, "wb") as f:
                if archive_format == "tar":
                    _write_tar(entries, f)
                else:
                    # The empty filename keeps the output's name out of the gzip header.
                    with gzip.GzipFile(
                        filename="",
                        mode="wb",
                        compresslevel=_DEFLATE_LEVEL,
                        fileobj=f,
                        mtime=0,
                    ) as gz:
                        _write_tar(entries, gz)
        if sha256 is not None:
            _write_sha256(out, sha256)


def compress(
    src: Path, out: Path, zstd: Sequence[str], sha256: Path | None = None
) -> None:
    """Compress `src` to `out` with the zstd command `zstd`; see `COMPRESS_HELP`."""
    if not zstd:
        raise ValueError("no zstd command")
    with _removed_on_failure(out, sha256):
        subprocess.run([*zstd, "-o", os.fspath(out), os.fspath(src)], check=True)
        if sha256 is not None:
            _write_sha256(out, sha256)


_DECLARATION_SUFFIXES = (
    ".d.ts",
    ".d.mts",
    ".d.cts",
    ".d.ts.map",
    ".d.mts.map",
    ".d.cts.map",
)


def strip_types(src: Path, out: Path) -> None:
    """Copy `src` into `out` without TypeScript declarations or their source maps.

    Symlinks are copied as their targets, so declarations behind a linked file
    or directory are stripped too. Raises `ValueError` on a symlink cycle.
    """
    _copy_resolved(src, out, set(), _DECLARATION_SUFFIXES)


def _copy_resolved(
    src: Path,
    out: Path,
    ancestors: set[tuple[int, int]],
    skip_suffixes: tuple[str, ...] = (),
) -> None:
    """Copy `src` into `out` with symlinks copied as their targets, leaving out files whose names, or whose symlink targets' names, end with one of `skip_suffixes`."""
    key = _enter_dir(src, ancestors)
    out.mkdir(parents=True, exist_ok=True)
    with os.scandir(src) as entries:
        for entry in entries:
            if entry.is_dir():
                _copy_resolved(
                    Path(entry.path), out / entry.name, ancestors, skip_suffixes
                )
            elif not entry.name.endswith(skip_suffixes) and not (
                entry.is_symlink()
                and os.path.realpath(entry.path).endswith(skip_suffixes)
            ):
                shutil.copy(entry.path, out / entry.name)
    ancestors.remove(key)


def manifest(tree: Path) -> dict[str, object]:
    """Describe each top-level and scoped package of the `node_modules` directory `tree`.

    Each package lists its `name` and, from its package.json, `version`, `os`,
    `cpu`, `libc`, `bin` (a string `bin` becomes an object keyed by the
    unscoped name) and `dependencies`: the names of its dependencies, optional
    dependencies and peer dependencies. Nested `node_modules` belong to their
    package and are not listed. A package reached through a symlink (or in a
    linked scope) records the link's target as `link` and, when it resolves to
    a file or directory inside `tree`, its `/`-separated path as `path`, or
    `outside` when it resolves to `tree` itself or outside it. An entry that is,
    or resolves inside `tree` to, a file records `file`. Dot-prefixed entries such as
    `.bin` and `.yarn-integrity`, at the top level or in a scope, are not
    packages; their names (`@scope/.name` in a scope) are listed under
    `ignored`. A package without a package.json is described by name only; one
    that cannot be read, or whose package.json is not a JSON object, is
    described by name and an `error` saying why. A scope directory that cannot
    be listed is described the same way under its name.
    """
    root = os.path.realpath(tree)
    packages = []
    ignored = []
    for entry in _sorted_entries(tree):
        if entry.name.startswith("."):
            ignored.append(entry.name)
            continue
        try:
            scope = _scope_entries(entry)
        except OSError as e:
            packages.append({"name": entry.name, "error": str(e)})
            continue
        if scope is None:
            packages.append(_describe_package(root, entry.name, entry, None))
            continue
        children, scope_link = scope
        for child in children:
            name = f"{entry.name}/{child.name}"
            if child.name.startswith("."):
                ignored.append(name)
            else:
                packages.append(_describe_package(root, name, child, scope_link))
    return {"packages": packages, "ignored": ignored}


def _scope_entries(
    entry: os.DirEntry[str],
) -> tuple[list[os.DirEntry[str]], str | None] | None:
    """The entries of the scope directory `entry` and the target it links to, or None when `entry` is not a scope."""
    if not (entry.name.startswith("@") and entry.is_dir()):
        return None
    scope_link = _link(entry) if entry.is_symlink() else None
    return _sorted_entries(Path(entry.path)), scope_link


def _sorted_entries(directory: Path) -> list[os.DirEntry[str]]:
    with os.scandir(directory) as entries:
        return sorted(entries, key=lambda e: e.name)


def _link(entry: os.DirEntry[str]) -> str:
    return os.readlink(entry.path).replace(os.sep, "/")


def _path_inside(root: str, real: str) -> str | None:
    try:
        inside = os.path.relpath(real, root).replace(os.sep, "/")
    except ValueError:  # On another Windows drive.
        return None
    if inside in (".", "..") or inside.startswith("../"):
        return None
    return inside


def _record_location(
    info: dict[str, object],
    root: str,
    entry: os.DirEntry[str],
    scope_link: str | None,
) -> bool:
    """Records where `entry` lives in `info`; whether it is a package directory to read."""
    link = _link(entry) if entry.is_symlink() else scope_link
    if link is None:
        if not entry.is_dir():
            info["file"] = True
        return entry.is_dir()
    info["link"] = link
    real = os.path.realpath(entry.path)
    is_file = os.path.isfile(real)
    if not is_file and not os.path.isdir(real):
        return False
    inside = _path_inside(root, real)
    if inside is None:
        info["outside"] = True
        return False
    info["path"] = inside
    if is_file:
        info["file"] = True
    return not is_file


def _describe_package(
    root: str, name: str, entry: os.DirEntry[str], scope_link: str | None
) -> dict[str, object]:
    info: dict[str, object] = {"name": name}
    try:
        if not _record_location(info, root, entry, scope_link):
            return info
        data = _read_json_object(Path(entry.path) / "package.json")
    except (OSError, ValueError) as e:
        info["error"] = str(e)
        return info
    for key in ("version", "os", "cpu", "libc"):
        if key in data:
            info[key] = data[key]
    bins = data.get("bin")
    if isinstance(bins, str):
        bins = {name.split("/")[-1]: bins}
    if isinstance(bins, dict) and bins:
        info["bin"] = bins
    dependencies = {
        dependency
        for key in ("dependencies", "optionalDependencies", "peerDependencies")
        if isinstance(data.get(key), dict)
        for dependency in data[key]
    }
    if dependencies:
        info["dependencies"] = sorted(dependencies)
    return info


def _read_json_object(path: Path) -> dict[str, object]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}
    except (OSError, ValueError) as e:
        raise ValueError(f"cannot read '{path}': {e}") from e
    if not isinstance(data, dict):
        raise ValueError(f"'{path}' is not a JSON object")
    return data


# Node's legacy `main` lookup finds the file at the `main` path plus any of these.
_MAIN_SUFFIXES = (
    "",
    ".js",
    ".json",
    ".node",
    "/index.js",
    "/index.json",
    "/index.node",
)


def validate_package(
    package: Path,
    name: str,
    dir_name: str | None,
    main: str | None,
    bins: dict[str, str],
    type_: str | None,
    version: str | None,
    module: Path | None = None,
    module_src_root: str = "",
) -> list[str]:
    """Return a message for each problem with the `package.json` of the package directory `package`.

    Its "name" must be `name` or `dir_name`. Its "main", "bin", "types",
    "typings" and literal "exports" targets must be files in the package, as
    Node, npm and TypeScript resolve them, and every "bin" file must start
    with a shebang, after any UTF-8 byte order mark. Every conditions object in "exports" must list "types"
    first. `main`, when set, must resolve to the same file as its "main", and
    `bins`, `type_` and `version`, when set, must match its "bin", "type" and
    "version". Its "type" must match that of the
    `package.json` nearest `module_src_root` inside `module`, the project
    directory `tsc` compiled the package from, when there is one.
    Paths match only with the same case, on any file system.
    """
    if not _is_file(package, "package.json"):
        return [
            "the package has no package.json: a `prebuilt` directory must contain one, or `package_json` adds it"
        ]
    try:
        manifest = json.loads((package / "package.json").read_text(encoding="utf-8"))
    except OSError as e:
        return [f"package.json cannot be read: {e.strerror or e}"]
    except ValueError as e:
        return [f"package.json is not valid JSON: {e}"]
    if not isinstance(manifest, dict):
        return ["package.json must contain a JSON object"]
    errors = []
    _check_name(manifest, name, dir_name, errors)
    if "main" in manifest:
        _check_main(package, manifest["main"], errors)
    reported = len(errors)
    commands = _bin_commands(manifest, errors)
    bin_malformed = len(errors) > reported
    _check_bins(package, commands, errors)
    for field in ("types", "typings"):
        if field in manifest:
            _check_types(package, field, manifest[field], errors)
    if "exports" in manifest:
        _check_exports(package, manifest["exports"], errors)
    _check_declared(
        package,
        manifest,
        commands,
        main,
        bins,
        bin_malformed,
        type_,
        version,
        errors,
    )
    if module is not None:
        _check_module_type(manifest, module, module_src_root, errors)
    return errors


def _check_declared(
    package: Path,
    manifest: dict[str, object],
    commands: dict[str, str],
    main: str | None,
    bins: dict[str, str],
    bin_malformed: bool,
    type_: str | None,
    version: str | None,
    errors: list[str],
) -> None:
    manifest_main = manifest.get("main")
    if main is not None and (
        not isinstance(manifest_main, str)
        or _main_target(package, manifest_main) != _main_target(package, main)
    ):
        errors.append(_mismatch(manifest, "main", main))
    compared_bins = (
        _well_formed_bins(manifest, commands, bins) if bin_malformed else bins
    )
    if compared_bins and _package_paths(commands) != _package_paths(compared_bins):
        errors.append(_mismatch(manifest, "bin", bins))
    if type_ is not None and manifest.get("type", "commonjs") != type_:
        errors.append(_mismatch(manifest, "type", type_))
    if version is not None and manifest.get("version") != version:
        errors.append(_mismatch(manifest, "version", version))


def _well_formed_bins(
    manifest: dict[str, object], commands: dict[str, str], bins: dict[str, str]
) -> dict[str, str]:
    """The entries of `bins` except those for commands whose "bin" entry is malformed, all of them when "bin" itself is."""
    value = manifest.get("bin")
    if not isinstance(value, dict):
        return {}
    return {
        command: target
        for command, target in bins.items()
        if command in commands or command not in value
    }


def _check_types(package: Path, field: str, value: object, errors: list[str]) -> None:
    if not isinstance(value, str):
        errors.append(f'package.json "{field}" must be a string, not {_show(value)}')
        return
    path = _package_path(value)
    candidates = (
        [path, path + ".d.ts", path + "/index.d.ts"] if path else ["index.d.ts"]
    )
    if not any(_is_file(package, candidate) for candidate in candidates):
        errors.append(
            f'package.json "{field}" {_show(value)} is not a file in the package'
        )


def _check_module_type(
    manifest: dict[str, object], module: Path, src_root: str, errors: list[str]
) -> None:
    segments = src_root.split("/") if src_root else []
    for i in range(len(segments), -1, -1):
        path = "/".join(segments[:i] + ["package.json"])
        if _is_file(module, path):
            break
    else:
        return
    try:
        module_manifest = json.loads((module / path).read_text(encoding="utf-8"))
    except OSError as e:
        errors.append(f"the module's {path} cannot be read: {e.strerror or e}")
        return
    except ValueError as e:
        errors.append(f"the module's {path} is not valid JSON: {e}")
        return
    if not isinstance(module_manifest, dict):
        errors.append(f"the module's {path} must contain a JSON object")
        return
    module_type = module_manifest.get("type", "commonjs")
    package_type = manifest.get("type", "commonjs")
    if package_type != module_type:
        errors.append(
            f'package.json "type" {_show(package_type)} does not match the "type" {_show(module_type)} '
            f"of the module's {path}, which sets the module format `tsc` emits for sources without a closer package.json; "
            'set `type`, or the "type" of `package_json`, to match'
        )


def _check_name(
    manifest: dict[str, object], name: str, dir_name: str | None, errors: list[str]
) -> None:
    names = [name] + ([dir_name] if dir_name is not None and dir_name != name else [])
    expected = " or ".join(
        f"the `{attr}` '{value}'" for attr, value in zip(["package_name", "out"], names)
    )
    if "name" not in manifest:
        errors.append(f'package.json has no "name"; it must be {expected}')
    elif manifest["name"] not in names:
        errors.append(
            f'package.json "name" {_show(manifest["name"])} must be {expected}'
        )


def _check_main(package: Path, value: object, errors: list[str]) -> None:
    if not isinstance(value, str):
        errors.append(f'package.json "main" must be a string, not {_show(value)}')
        return
    if _main_file(package, value) is None:
        errors.append(
            f'package.json "main" {_show(value)} is not a file in the package'
        )


def _check_bins(package: Path, commands: dict[str, str], errors: list[str]) -> None:
    for command, target in commands.items():
        path = _package_path(target)
        if not _is_file(package, path):
            errors.append(
                f'package.json "bin" {_show(command)} {_show(target)} is not a file in the package'
            )
        else:
            try:
                has_shebang = _starts_with_shebang(package / path)
            except OSError as e:
                errors.append(
                    f'package.json "bin" {_show(command)} {_show(target)}: cannot read: {e.strerror or e}'
                )
                continue
            if not has_shebang:
                errors.append(
                    f"package.json \"bin\" {_show(command)} {_show(target)} does not start with a shebang ('#!')"
                )


def _show(value: object) -> str:
    return f"'{value}'" if isinstance(value, str) else json.dumps(value)


def _mismatch(manifest: dict[str, object], field: str, declared: object) -> str:
    if field not in manifest:
        return f'package.json has no "{field}", but `{field}` is {_show(declared)}'
    return f'package.json "{field}" {_show(manifest[field])} does not match `{field}` {_show(declared)}'


def _package_path(value: str) -> str:
    """The normalized package-relative path that a path in package.json names, `""` for the package root.

    A path that leaves the package starts with `..` or `/`, which no file in the package matches.
    """
    path = posixpath.normpath(value)
    return "" if path == "." else path


def _package_paths(targets: dict[str, str]) -> dict[str, str]:
    return {key: _package_path(target) for key, target in targets.items()}


def _main_file(package: Path, value: str) -> str | None:
    path = _package_path(value)
    if path:
        candidates = [path + suffix for suffix in _MAIN_SUFFIXES]
    else:
        candidates = ["index.js", "index.json", "index.node"]
    return next(
        (candidate for candidate in candidates if _is_file(package, candidate)), None
    )


def _main_target(package: Path, value: str) -> str:
    return _main_file(package, value) or _package_path(value)


def _bin_commands(manifest: dict[str, object], errors: list[str]) -> dict[str, str]:
    """The command names and targets of "bin"; a string names one command after the package."""
    value = manifest.get("bin")
    if value is None:
        return {}
    if isinstance(value, str):
        name = manifest.get("name")
        if not isinstance(name, str):
            errors.append(
                'package.json "bin" is a string, which names its command after "name", so "name" must be a string'
            )
            return {}
        return {name.split("/")[-1]: value}
    if not isinstance(value, dict):
        errors.append('package.json "bin" must be a string or an object')
        return {}
    commands = {}
    for command, target in value.items():
        if isinstance(target, str):
            commands[command] = target
        else:
            errors.append(
                f'package.json "bin" {_show(command)} must be a string, not {_show(target)}'
            )
    return commands


def _check_exports(package: Path, exports: object, errors: list[str]) -> None:
    if isinstance(exports, dict):
        subpaths = [key for key in exports if key.startswith(".")]
        if subpaths and len(subpaths) != len(exports):
            errors.append(
                "package.json \"exports\" mixes subpaths (keys starting with '.') and conditions"
            )
            return
        if subpaths:
            for key, target in exports.items():
                _check_export_target(
                    package, target, f"exports[{json.dumps(key)}]", errors
                )
            return
    _check_export_target(package, exports, "exports", errors)


def _check_export_target(
    package: Path, target: object, where: str, errors: list[str]
) -> None:
    if target is None:
        return
    if isinstance(target, str):
        if "*" in target:
            return
        if not target.startswith("./"):
            errors.append(f"package.json {where} {_show(target)} must start with './'")
            return
        if not _is_file(package, _package_path(target)):
            errors.append(
                f"package.json {where} {_show(target)} is not a file in the package"
            )
    elif isinstance(target, list):
        for i, fallback in enumerate(target):
            _check_export_target(package, fallback, f"{where}[{i}]", errors)
    elif isinstance(target, dict):
        conditions = list(target)
        if "types" in conditions and conditions[0] != "types":
            errors.append(
                f'package.json {where} lists "types" after "{conditions[0]}"; "types" must come first'
            )
        for condition, nested in target.items():
            _check_export_target(
                package, nested, f"{where}[{json.dumps(condition)}]", errors
            )
    else:
        errors.append(
            f"package.json {where} must be a string, an array, an object or null"
        )


def _is_file(package: Path, path: str) -> bool:
    """Whether the package-relative `path` is a file in `package`, matching the case of every segment."""
    entry = None
    for segment in path.split("/"):
        directory = package if entry is None else entry.path
        try:
            with os.scandir(directory) as entries:
                entry = next((e for e in entries if e.name == segment), None)
        except OSError:
            return False
        if entry is None:
            return False
    return entry.is_file()


def _starts_with_shebang(path: Path) -> bool:
    with open(path, "rb") as f:
        return f.read(5).removeprefix(b"\xef\xbb\xbf")[:2] == b"#!"


def run_build(
    staged: Path,
    output_path: str,
    out: Path,
    command: Sequence[str],
    work_parent: Path,
    root_prefix: str = "",
    env_keys: Collection[str] = (),
) -> int:
    """Run `command` in a copy of `staged` and copy the directory it writes at `output_path` to `out`.

    The copy is made in `work_parent`. Files outside `node_modules` are
    copied, so the command may write beside them; each entry of
    `node_modules` is a symlink to the staged one, so packages resolve to
    their staged location. In `command` and in the environment variables
    `env_keys`, `root_prefix` is replaced by the current directory, which
    the command no longer runs in. Symlinks are copied as their targets,
    both into the copy and to `out`. Returns the command's exit code, or 1
    if it wrote no `output_path` directory. Raises `ValueError` on a
    symlink cycle and `OSError` if a symlink cannot be made or a file
    cannot be copied.
    """
    root = os.getcwd() + os.sep
    work = work_parent / "work"
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)
    for child in sorted(staged.iterdir()):
        if child.name == "node_modules":
            (work / child.name).mkdir()
            for package in sorted(child.iterdir()):
                target = package.resolve()
                (work / child.name / package.name).symlink_to(
                    target, target_is_directory=target.is_dir()
                )
        elif child.is_dir():
            _copy_resolved(child, work / child.name, set())
        else:
            shutil.copy(child, work / child.name)
    env = dict(os.environ)
    if root_prefix:
        command = [arg.replace(root_prefix, root) for arg in command]
        for key in env_keys:
            if key in env:
                env[key] = env[key].replace(root_prefix, root)
    code = subprocess.run(command, cwd=work, env=env).returncode
    if code != 0:
        return code
    built = work / output_path
    if not built.is_dir():
        print(
            f"build: the command wrote no directory at '{output_path}'",
            file=sys.stderr,
        )
        return 1
    _copy_resolved(built, out, set())
    return 0


def write_validation_result(out: Path, errors: list[str]) -> None:
    """Write `errors` as a Buck2 validation result: a failure listing them, or a success when there are none."""
    data = (
        {"status": "failure", "message": "\n".join(errors)}
        if errors
        else {"status": "success"}
    )
    out.write_text(json.dumps({"version": 1, "data": data}), encoding="utf-8")


def _bin_arg(arg: str) -> tuple[str, str]:
    command, sep, path = arg.partition(":")
    if not sep:
        raise argparse.ArgumentTypeError(f"expected NAME:PATH, got {arg!r}")
    return command, path


def _path(arg: str) -> Path:
    # `node_modules` trees routinely exceed MAX_PATH on Windows.
    if sys.platform == "win32":
        return Path("\\\\?\\" + os.path.abspath(arg))
    return Path(arg)


def _read_executables(path: Path | None) -> list[str]:
    if path is None:
        return []
    lines = path.read_text(encoding="utf-8").splitlines()
    return [line.strip() for line in lines if line.strip()]


def _run_assemble_package(args: argparse.Namespace) -> int:
    if args.prebuilt and args.package_subdir:
        raise ValueError("--package-subdir cannot be combined with --prebuilt")
    entries = json.loads(args.entries.read_text(encoding="utf-8"))
    errors = assemble_package(
        args.prebuilt or args.emit,
        args.package_subdir,
        [(path, _path(src), name) for path, src, name in entries],
        args.package_json,
        args.out,
        origin="prebuilt" if args.prebuilt else "emitted",
    )
    for error in errors:
        print(error, file=sys.stderr)
    return 1 if errors else 0


def _run_validate_package(args: argparse.Namespace) -> int:
    write_validation_result(
        args.out,
        validate_package(
            args.package,
            args.name,
            args.dir_name,
            args.main,
            dict(args.bin),
            args.type,
            args.version,
            args.module,
            args.module_src_root,
        ),
    )
    return 0


def _run_manifest(args: argparse.Namespace) -> int:
    args.out.write_text(
        json.dumps(manifest(args.tree), indent=1, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return 0


def _run_launchers(args: argparse.Namespace) -> int:
    write_launchers(json.loads(args.spec.read_text(encoding="utf-8")), args.out)
    return 0


def _run_archive(args: argparse.Namespace) -> int:
    write_archive(
        args.src,
        args.format,
        args.out,
        args.root,
        _read_executables(args.executables),
        args.sha256,
    )
    return 0


def _run_compress(args: argparse.Namespace) -> int:
    compress(args.src, args.out, args.zstd, args.sha256)
    return 0


def _run_build(args: argparse.Namespace) -> int:
    if not args.command_args:
        raise ValueError("--command requires a command")
    scratch = os.environ.get("BUCK_SCRATCH_PATH")
    if scratch:
        return run_build(
            args.dir,
            args.output_path,
            args.out,
            args.command_args,
            _path(scratch),
            args.root_prefix,
            args.env_key,
        )
    with tempfile.TemporaryDirectory() as tmp:
        return run_build(
            args.dir,
            args.output_path,
            args.out,
            args.command_args,
            Path(tmp),
            args.root_prefix,
            args.env_key,
        )


def _run_strip_types(args: argparse.Namespace) -> int:
    strip_types(args.src, args.out)
    return 0


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    assemble = commands.add_parser("assemble-package")
    base = assemble.add_mutually_exclusive_group(required=True)
    base.add_argument("--emit", type=_path)
    base.add_argument("--prebuilt", type=_path)
    assemble.add_argument("--package-subdir", default="")
    assemble.add_argument("--entries", type=_path, required=True)
    assemble.add_argument("--package-json", type=_path)
    assemble.add_argument("--out", type=_path, required=True)
    assemble.set_defaults(run=_run_assemble_package, errors=(ValueError, OSError))
    described = commands.add_parser("manifest")
    described.add_argument("--tree", type=_path, required=True)
    described.add_argument("--out", type=_path, required=True)
    described.set_defaults(run=_run_manifest, errors=(ValueError, OSError))
    launchers = commands.add_parser(
        "launchers",
        description=LAUNCHERS_HELP,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    launchers.add_argument("--spec", type=_path, required=True, metavar="FILE")
    launchers.add_argument("--out", type=_path, required=True, metavar="DIR")
    launchers.set_defaults(run=_run_launchers, errors=(OSError, ValueError))
    archive = commands.add_parser(
        "archive",
        description=ARCHIVE_HELP,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    archive.add_argument("--src", type=_path, required=True, metavar="DIR")
    archive.add_argument("--format", choices=ARCHIVE_FORMATS, required=True)
    archive.add_argument("--out", type=_path, required=True, metavar="FILE")
    archive.add_argument("--root", default="")
    archive.add_argument("--executables", type=_path, metavar="FILE")
    archive.add_argument("--sha256", type=_path, metavar="FILE")
    archive.set_defaults(run=_run_archive, errors=(ValueError, OSError))
    compressed = commands.add_parser(
        "compress",
        description=COMPRESS_HELP,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    compressed.add_argument("--src", type=_path, required=True, metavar="FILE")
    compressed.add_argument("--out", type=_path, required=True, metavar="OUT")
    compressed.add_argument("--sha256", type=_path, metavar="FILE")
    compressed.add_argument("--zstd", nargs=argparse.REMAINDER, required=True)
    compressed.set_defaults(
        run=_run_compress,
        errors=(ValueError, OSError, subprocess.CalledProcessError),
    )
    build = commands.add_parser("build")
    build.add_argument("--dir", type=_path, required=True)
    build.add_argument("--output-path", required=True)
    build.add_argument("--out", type=_path, required=True)
    build.add_argument("--root-prefix", default="")
    build.add_argument("--env-key", action="append", default=[])
    build.add_argument(
        "--command", dest="command_args", nargs=argparse.REMAINDER, required=True
    )
    build.set_defaults(run=_run_build, errors=(ValueError, OSError))
    strip = commands.add_parser("strip-types")
    strip.add_argument("--src", type=_path, required=True)
    strip.add_argument("--out", type=_path, required=True)
    strip.set_defaults(run=_run_strip_types, errors=(ValueError, OSError))
    validate = commands.add_parser("validate-package")
    validate.add_argument("--package", type=_path, required=True)
    validate.add_argument("--name", required=True)
    validate.add_argument("--dir-name")
    validate.add_argument("--main")
    validate.add_argument(
        "--bin", type=_bin_arg, action="append", default=[], metavar="NAME:PATH"
    )
    validate.add_argument("--type")
    validate.add_argument("--version")
    validate.add_argument("--module", type=_path)
    validate.add_argument("--module-src-root", default="")
    validate.add_argument("--out", type=_path, required=True)
    validate.set_defaults(run=_run_validate_package, errors=(ValueError, OSError))
    return parser


def main() -> int:
    args = _parser().parse_args()
    try:
        return args.run(args)
    except args.errors as e:
        print(f"{args.command}: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
