#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""
For certain platform and linker versions (e.g. LLD 15+ for ELF), the
linker ignores the archive symbol table and accesses archive members
directly. When combined with thin archives, this produces trivial
archive files just embedding object paths, but ar (both GNU and LLVM)
still requires access to the object files to produce the archive,
resulting in unnecessary downloads and RE traffic. This implementation
instead embeds the paths directly without needing the actual files
present. The trade-offs are:
- Any problems in the object files will be detected at link time instead
  of archive creation time. This should be very rare though.
- Since we can't access the object files, we store their sizes as zero
  instead of the actual file size in the archive member headers. LLD for
  ELF handles this correctly but I can't speak to other linkers.
"""

import argparse
import os.path
import typing as t
from pathlib import Path


class ThinArchive:
    MAGIC = b"!<thin>\n"

    def __init__(self, inputs: t.Sequence[Path], output: Path) -> None:
        self._output = output
        # llvm-ar always uses the long name member, and we follow suit for simplicity.
        self._create_name_data(inputs, output.parent)

    def write(self) -> None:
        with self._output.open("wb") as archive:
            archive.write(self.MAGIC)
            self._write_member_header(
                archive,
                name="//",
                mtime="",
                owner_id="",
                group_id="",
                mode="",
                size=len(self._name_data),
            )
            archive.write(self._name_data)

            for offset in self._name_offsets:
                self._write_member_header(
                    archive,
                    name=f"/{offset}",
                    mtime="0",
                    owner_id="0",
                    group_id="0",
                    mode="644",
                    size=0,  # as discussed in the file docblock
                )

    def _create_name_data(self, inputs: t.Sequence[Path], output_dir: Path) -> None:
        self._name_data = bytearray()
        self._name_offsets = []
        for input_path in inputs:
            # Paths are stored relative to the archive. We use os.path.relpath instead
            # of Pathlib.relative_to because the latter requires a common root. We use
            # forward slashes everywhere for consistency and to mimic llvm-ar.
            relative_path = Path(os.path.relpath(input_path, output_dir)).as_posix()
            encoded = (relative_path + "/\n").encode()  # add terminator
            self._name_offsets.append(len(self._name_data))
            self._name_data.extend(encoded)

        if len(self._name_data) % 2 != 0:
            self._name_data.extend(b"\n")  # pad to an even size

    def _write_member_header(
        self,
        archive: t.BinaryIO,
        *,
        name: str,
        mtime: str,
        owner_id: str,
        group_id: str,
        mode: str,
        size: int,
    ) -> None:
        # https://en.wikipedia.org/wiki/Ar_(Unix)#File_header
        archive.write(self._encode_header_field(name, 16))
        archive.write(self._encode_header_field(mtime, 12))
        archive.write(self._encode_header_field(owner_id, 6))
        archive.write(self._encode_header_field(group_id, 6))
        archive.write(self._encode_header_field(mode, 8))
        archive.write(self._encode_header_field(str(size), 10))
        archive.write(b"`\n")  # ending characters

    def _encode_header_field(self, value: str, length: int) -> bytes:
        encoded = value.encode()
        padding = length - len(encoded)
        if padding < 0:
            raise ValueError(f"Encoding of {str} is larger than {length} bytes")

        return encoded + b" " * padding


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Simple thin archive creator", fromfile_prefix_chars="@"
    )
    parser.add_argument("modifiers", help="Operation and modifiers (limited support)")
    parser.add_argument("output", type=Path, help="The output file")
    parser.add_argument("inputs", nargs="+", help="The input files")
    args = parser.parse_args()

    if args.output.exists():
        raise ValueError(
            f"Appending to an existing archive ({args.output}) is unsupported"
        )

    thin = False
    for modifier in args.modifiers:
        if modifier == "s":
            raise ValueError("Archive symbol tables are unsupported")
        elif modifier == "T":
            thin = True
        elif modifier not in "qcSD":
            raise ValueError(f"Unsupported operation or modifier {modifier}")

    if not thin:
        raise ValueError("Only thin archives are supported")

    # Strip any leading or trailing quotes (present in Windows argsfiles)
    inputs = [Path(p.lstrip('"').rstrip('"')) for p in args.inputs]
    archive = ThinArchive(inputs, args.output)
    archive.write()


if __name__ == "__main__":
    main()
