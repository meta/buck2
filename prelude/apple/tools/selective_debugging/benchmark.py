#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
Times `scrub()` on a synthetic Mach-O file, or on a real binary with --input.

The synthetic default is shaped like a large iOS framework: 28k N_OSO paths over 3k
targets, most of them objects under `__objects__`, and 11M other symbols for the
scrubber to skip.

    buck2 run prelude//apple/tools/selective_debugging:benchmark
    buck2 run prelude//apple/tools/selective_debugging:benchmark -- \\
        --input <binary> --targets-file <focused_targets.json>
"""

import argparse
import json
import os
import statistics
import struct
import tempfile
import time
from collections.abc import Callable

from .macho import LC_SEGMENT_64, LC_SYMTAB, MH_MAGIC_64, N_OSO
from .scrubber import scrub

_HEADER = struct.Struct("<IiiIIIII")
_SEGMENT_64 = struct.Struct("<II16sQQQQiiII")
_SYMTAB = struct.Struct("<IIIIII")
_NLIST_64 = struct.Struct("<IBBHQ")
_N_SECT = 0x0E
_MH_DYLIB = 0x6
_OUTPUT_DIR = "buck-out/v2/gen/root/0123456789abcdef/src"


def _target(index: int) -> tuple[str, str]:
    return f"src/project{index % 97}", f"Target{index}"


def _oso_paths(num_targets: int, objects_per_target: int) -> list[str]:
    paths = []
    for t in range(num_targets):
        package, name = _target(t)
        out = f"{_OUTPUT_DIR}/project{t % 97}/__{name}__"
        for o in range(objects_per_target):
            if t % 30 == 0:
                paths.append(f"{out}/lib{name}.a(File{o}.cpp.o)")
            else:
                paths.append(f"{out}/__objects__/dir{o % 3}/File{o}.mm.o")
    return paths


def _write_macho(path: str, oso_paths: list[str], num_other_symbols: int) -> None:
    """
    Writes a minimal 64-bit Mach-O: a header, LC_SEGMENT_64 for __LINKEDIT and
    LC_SYMTAB, then the symbol table and the string table.
    """
    strtab = bytearray(b" \x00")
    other_index = len(strtab)
    strtab += b"_symbol\x00"
    oso_indexes = []
    for oso_path in oso_paths:
        oso_indexes.append(len(strtab))
        strtab += oso_path.encode() + b"\x00"

    n_symbols = len(oso_paths) + num_other_symbols
    commands_size = _SEGMENT_64.size + _SYMTAB.size
    symtab_offset = _HEADER.size + commands_size
    strtab_offset = symtab_offset + n_symbols * _NLIST_64.size
    linkedit_size = strtab_offset + len(strtab) - symtab_offset

    with open(path, "wb") as f:
        f.write(_HEADER.pack(MH_MAGIC_64, 0, 0, _MH_DYLIB, 2, commands_size, 0, 0))
        f.write(
            _SEGMENT_64.pack(
                LC_SEGMENT_64,
                _SEGMENT_64.size,
                b"__LINKEDIT",
                0,
                linkedit_size,
                symtab_offset,
                linkedit_size,
                1,
                1,
                0,
                0,
            )
        )
        f.write(
            _SYMTAB.pack(
                LC_SYMTAB,
                _SYMTAB.size,
                symtab_offset,
                n_symbols,
                strtab_offset,
                len(strtab),
            )
        )
        for index in oso_indexes:
            f.write(_NLIST_64.pack(index, N_OSO, 0, 1, 0))
        f.write(_NLIST_64.pack(other_index, _N_SECT, 1, 0, 0) * num_other_symbols)
        f.write(strtab)


def _int_at_least(minimum: int) -> Callable[[str], int]:
    def integer(value: str) -> int:
        number = int(value)
        if number < minimum:
            raise argparse.ArgumentTypeError(
                f"must be at least {minimum}, got {number}"
            )
        return number

    return integer


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--targets", type=_int_at_least(1), default=3150)
    parser.add_argument("--objects-per-target", type=_int_at_least(1), default=9)
    parser.add_argument("--other-symbols", type=_int_at_least(0), default=11_350_000)
    parser.add_argument("--focused-targets", type=_int_at_least(1), default=14)
    parser.add_argument("--iterations", type=_int_at_least(1), default=5)
    parser.add_argument("--input", help="Binary to scrub instead of a synthetic one")
    parser.add_argument("--targets-file", help="Targets file to scrub --input with")
    return parser.parse_args()


def main() -> None:
    args = _parse_args()
    with tempfile.TemporaryDirectory() as tmp_dir:
        input_path = args.input
        targets_file = args.targets_file
        if not input_path:
            input_path = os.path.join(tmp_dir, "input")
            oso_paths = _oso_paths(args.targets, args.objects_per_target)
            _write_macho(input_path, oso_paths, args.other_symbols)
            print(
                f"Synthetic binary: {len(oso_paths)} N_OSO paths, "
                f"{args.other_symbols} other symbols"
            )
        if not targets_file:
            targets_file = os.path.join(tmp_dir, "focused_targets.json")
            step = max(1, args.targets // args.focused_targets)
            focused = [_target(t) for t in range(0, args.targets, step)]
            with open(targets_file, "w") as f:
                json.dump(
                    {
                        "targets": [
                            f"root//{package}:{name}"
                            for package, name in focused[: args.focused_targets]
                        ]
                    },
                    f,
                )

        output_path = os.path.join(tmp_dir, "output")
        timings = []
        for _ in range(args.iterations):
            start = time.perf_counter()
            results = scrub(input_path, output_path, None, targets_file=targets_file)
            timings.append(time.perf_counter() - start)
            os.remove(output_path)

    kept = sum(1 for original, rewritten in results if original == rewritten)
    print(f"Kept {kept} of {len(results)} N_OSO paths")
    print(
        f"scrub(): median {statistics.median(timings):.3f}s, "
        f"min {min(timings):.3f}s over {len(timings)} runs"
    )


if __name__ == "__main__":
    main()
