#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# A "fake" linker command meant to be provided to rustc as `-Clinker={}`. This script extracts
# from rustc's linker invocation the link inputs that only rustc can provide — the objects it
# synthesized for this crate — and exports them as outputs to later be consumed by the cxx-driven
# link of this crate.
#
# Everything else on the argv is discarded: the link is performed by cxx, and every flag and
# library it needs is provided by the toolchain and the dependency graph, the same way it is for
# a `cxx_binary`. See `process_link_args()` for the exact contract.

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any, IO, NamedTuple


def eprint(*args: Any, **kwargs: Any) -> None:
    print(*args, end="\n", file=sys.stderr, flush=True, **kwargs)


class Args(NamedTuple):
    out_argsfile: IO[str]
    out_artifacts: Path
    out_manifest: IO[str]
    out_archive: Path | None
    archiver_argsfile: Path | None
    linker: list[str]


def arg_parse() -> Args:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--out_argsfile",
        type=argparse.FileType("w"),
        required=True,
    )
    parser.add_argument(
        "--out_artifacts",
        type=Path,
        required=True,
    )
    parser.add_argument(
        "--out_manifest",
        type=argparse.FileType("w"),
        required=True,
        help="Receives the basenames of the extracted objects, one per line",
    )
    # When --out_archive and archiver_argsfile are set, the rustc objects are
    # additionally collected into an archive. Distributed ThinLTO needs
    # the objects to arrive as an `ArchiveLinkable` so it can plan a per-object
    # opt action for each linkable.
    parser.add_argument(
        "--out_archive",
        type=Path,
        default=None,
    )
    parser.add_argument(
        "--archiver_argsfile",
        type=Path,
        default=None,
        help="File holding the archiver command, one argument per line",
    )
    parser.add_argument(
        "linker",
        nargs=argparse.REMAINDER,
        type=str,
        help="Linker command line",
    )

    return Args(**vars(parser.parse_args()))


def expand_response_files(args: list[str]) -> list[str]:
    """Inline the contents of any `@file` argument.

    When the argv would exceed the OS command-line limit (which a Rust link
    line always does on Windows), rustc re-invokes the linker with all
    arguments in a single `@file`, one argument per line. Three formats are
    seen in the wild:
     - UTF-16 with each argument quoted and embedded quotes doubled, for
       msvc-flavored linkers;
     - UTF-8 with the same quoting, for direct (non-cc-driver) linkers
       invoked on a Windows host, e.g. wasm-ld;
     - UTF-8 with backslashes and spaces escaped by a backslash, for
       gcc-flavored linkers.
    The quoted forms leave backslashes raw, so quoting is detected per line
    before falling back to backslash-unescaping. A backslash-escaped line
    can never begin with a bare quote (it would be written as `\\"`), so the
    detection is unambiguous.
    """
    expanded = []
    for arg in args:
        if not arg.startswith("@"):
            expanded.append(arg)
            continue
        data = Path(arg[1:]).read_bytes()
        utf16 = data.startswith(b"\xff\xfe") or data.startswith(b"\xfe\xff")
        text = data.decode("utf-16") if utf16 else data.decode("utf-8-sig")
        for line in text.splitlines():
            if not line:
                continue
            if len(line) >= 2 and line.startswith('"') and line.endswith('"'):
                expanded.append(line[1:-1].replace('""', '"'))
            elif utf16:
                expanded.append(line)
            else:
                expanded.append(re.sub(r"\\(.)", r"\1", line))
    return expanded


def process_link_args(
    args: list[str], out_artifacts: Path
) -> tuple[list[str], list[str]]:
    """Extract rustc's synthesized objects from its linker argv, returning
    (retained args, objects).

    The objects are the only thing extracted:
     - the codegen unit objects of the crate itself, which includes the
       allocator shim module when rustc generates one, and
     - `symbols.o`, which carries undefined references to the exported and
       `#[used]` symbols of every linked crate so that the corresponding
       archive members survive the link.

    The only args retained are wasm-ld `--export` args, which carry the same
    per-program symbol metadata that `symbols.o` does on ELF, and entry-point
    args (see the comments at the branches). All other flags and libraries rustc puts on its link line
    are deliberately dropped: the toolchain and the dependency graph are the
    sole owners of the flags and libraries on the cxx-driven link. A
    rustc-synthesized file we don't recognize is an error, not something to
    pass through: it would be a dangling path by the time the link action
    runs. Version scripts are one known such case, so dylib and cdylib crates
    cannot currently be linked through this.
    """
    retained_args = []
    objects = []
    # Original argv entries we handled, and the directories rustc placed
    # synthesized files in, used to detect files we don't know about.
    handled = set()
    temp_dirs = set()

    i = 0
    size = len(args)
    while i < size:
        arg = args[i]
        if arg.endswith("rcgu.o") or arg.endswith("symbols.o"):
            path = Path(arg)
            if path.parent.is_absolute():
                temp_dirs.add(str(path.parent))
            handled.add(arg)

            objects.append(shutil.copy(path, out_artifacts))
            i += 1
            continue

        # wasm has no `symbols.o`: rustc communicates the crate graph's
        # exported/kept-alive symbols to wasm-ld as `--export` arguments
        # instead. Retaining these is acceptable for the same reason
        # extracting `symbols.o` is — they are per-program symbol metadata,
        # the analogue of what C compilers embed in object files as wasm
        # export flags — NOT a precedent for preserving arbitrary flags.
        elif arg in ("--export", "--export-if-defined"):
            retained_args.append(arg)
            if i + 1 < size:
                handled.add(args[i + 1])
                retained_args.append(args[i + 1])
            i += 2
            continue
        elif arg.startswith("--export=") or arg.startswith("--export-if-defined="):
            retained_args.append(arg)
            i += 1
            continue

        # Entry-point metadata is the same category: rustc knows whether the
        # program's entry is a `_start`-style symbol or an exported `main`
        # with no native entry (`--no-entry`). In C that information is
        # carried by whether an object defines the entry symbol.
        elif arg == "--no-entry":
            retained_args.append(arg)
            i += 1
            continue
        elif arg == "--entry":
            retained_args.append(arg)
            if i + 1 < size:
                handled.add(args[i + 1])
                retained_args.append(args[i + 1])
            i += 2
            continue
        elif arg.startswith("--entry="):
            retained_args.append(arg)
            i += 1
            continue
        # The msvc spellings, which `#![windows_subsystem]` sets.
        elif arg.startswith("/SUBSYSTEM:") or arg.startswith("/ENTRY:"):
            retained_args.append(arg)
            i += 1
            continue

        # Debugger-visualizer files (crate-graph natvis, embedded into the
        # PDB by the linker). Deliberately dropped rather than extracted: the
        # natvis set is declarable in the dependency graph — the attribute
        # names an ordinary source file — and C++ treats visualizers as
        # build/debugger configuration, not compiler output. Marked handled
        # so the temp-file check stays quiet.
        elif arg.startswith("/NATVIS:"):
            handled.add(arg)
            i += 1
            continue

        # The msvc-style spelling of `-o` below; the temporary output path
        # is embedded in the token, so mark it handled.
        elif arg.startswith("/OUT:"):
            handled.add(arg)
            i += 1
            continue

        # `-o`'s value is a temporary output location that rustc would copy to
        # the `--emit=link={}` path; the link action provides its own output
        # path instead. `-L` carries only search paths (the dummy sysroot and
        # rustc's raw-dylibs temp directory), never a file. Both are marked
        # handled so their values don't trip the unrecognized-file check
        # below.
        elif arg == "-o" or arg == "-L":
            if i + 1 < size:
                handled.add(args[i + 1])
            i += 2  # also skip the flag's value
            continue

        i += 1

    # Anything left referencing a directory rustc synthesized files into is a
    # file this tool doesn't know about. It won't exist anymore when the link
    # action runs (possibly on another host), so it cannot be passed along;
    # fail loudly rather than silently dropping a link input.
    leaked = [a for a in args if a not in handled and any(d in a for d in temp_dirs)]
    if leaked:
        eprint(
            "rustc passed link inputs that extract_link_action.py does not know how to extract:"
        )
        for a in leaked:
            eprint(f"  {a}")
        eprint("Teach process_link_args() about them.")
        sys.exit(1)

    # rustc always passes at least the crate's own codegen-unit objects, so
    # extracting nothing means the argv was misparsed. This also backstops the
    # check above, which is blind when no extraction established `temp_dirs`.
    if not objects:
        eprint(
            "extract_link_action.py: extracted no objects from rustc's link line; teach process_link_args() about whatever form it took:"
        )
        for a in args:
            eprint(f"  {a!r}")
        sys.exit(1)

    return retained_args, objects


def archive_objects(
    archiver_argsfile: Path, out_archive: Path, objects: list[str]
) -> None:
    archiver = archiver_argsfile.read_text().split("\n")
    archiver = [arg for arg in archiver if arg]

    env = {**os.environ, "ZERO_AR_DATE": "1"}
    subprocess.check_call([*archiver, str(out_archive), *objects], env=env)


def main() -> int:
    args = arg_parse()

    if args.out_artifacts.exists():
        shutil.rmtree(args.out_artifacts)
    os.mkdir(args.out_artifacts)

    archiver_argsfile = args.archiver_argsfile
    out_archive = args.out_archive
    archiving = archiver_argsfile is not None and out_archive is not None

    retained_args, objects = process_link_args(
        expand_response_files(args.linker[1:]),
        out_artifacts=args.out_artifacts,
    )
    args.out_argsfile.write("\n".join(retained_args))
    args.out_argsfile.close()

    # The manifest carries only basenames: the consumer knows the artifacts
    # directory as an artifact and resolves members against it, so the
    # execution-time path of the directory never appears in any output.
    args.out_manifest.write("\n".join(os.path.basename(o) for o in objects))
    args.out_manifest.close()

    if archiving:
        archive_objects(archiver_argsfile, out_archive, objects)

    return 0


sys.exit(main())
