# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import struct
import sys
from typing import BinaryIO, Optional

from .macho import (
    LC_CODE_SIGNATURE,
    LC_SEGMENT_64,
    LC_SYMTAB,
    LinkEditCommand,
    MachOHeader,
    N_OSO,
    Symbol,
    SymtabCommand,
)

"""
// Each LC_SYMTAB entry consists of the following fields:
// - String Index: 4 bytes, offset into the string table (I == unsigned int)
// - Type: 1 byte (B == unsigned char)
// - Section: 1 byte, (B == unsigned char)
// - Description: 2 bytes (H == unsigned short)
// - Value: 8 bytes on 64bit, 4 bytes on 32bit (Q == unsigned long long)
// - Everything is little-endian representation (denoted by <)
"""
_LC_SYMTAB_STRUCT = struct.Struct("<IBBHQ")


def _read_bytes(f: BinaryIO, n_bytes: int) -> int:
    b = f.read(n_bytes)
    return int.from_bytes(b, "little")


def load_header(f: BinaryIO, offset: int) -> tuple[MachOHeader, int]:
    f.seek(offset)
    magic = _read_bytes(f, 4)
    cpu_type = _read_bytes(f, 4)
    cpu_sub_type = _read_bytes(f, 4)
    file_type = _read_bytes(f, 4)
    cmd_cnt = _read_bytes(f, 4)
    cmd_size = _read_bytes(f, 4)
    flags = _read_bytes(f, 4)
    reserved = _read_bytes(f, 4)
    header = MachOHeader(
        magic, cpu_type, cpu_sub_type, file_type, cmd_cnt, cmd_size, flags, reserved
    )
    return header, f.tell()


def load_commands(
    f: BinaryIO, offset: int, n_cmds: int
) -> tuple[Optional[LinkEditCommand], Optional[SymtabCommand]]:
    """
    The OSO entries are identified in segments named __LINKEDIT.
    If no segment is found with that name, there is nothing to scrub.
    """
    lc_linkedit = None
    lc_symtab = None
    f.seek(offset)
    for _ in range(n_cmds):
        pos = f.tell()
        cmd = _read_bytes(f, 4)
        size = _read_bytes(f, 4)
        if cmd == LC_SEGMENT_64:
            name = f.read(16)
            if "LINKEDIT" in name.decode():
                vm_addr = _read_bytes(f, 8)
                vm_size = _read_bytes(f, 8)
                file_offset = _read_bytes(f, 8)
                file_size = _read_bytes(f, 8)
                maximum_vm_protection = _read_bytes(f, 4)
                initial_vm_protection = _read_bytes(f, 4)
                sections = _read_bytes(f, 4)
                flags = _read_bytes(f, 4)
                lc_linkedit = LinkEditCommand(
                    cmd,
                    size,
                    name,
                    vm_addr,
                    vm_size,
                    file_offset,
                    file_size,
                    maximum_vm_protection,
                    initial_vm_protection,
                    sections,
                    flags,
                )
                continue
        elif cmd == LC_SYMTAB:
            symtab_offset = _read_bytes(f, 4)
            n_symbols = _read_bytes(f, 4)
            strtab_offset = _read_bytes(f, 4)
            strtab_size = _read_bytes(f, 4)
            lc_symtab = SymtabCommand(
                cmd, size, symtab_offset, n_symbols, strtab_offset, strtab_size
            )
            continue
        elif cmd == LC_CODE_SIGNATURE:
            print("[Focused Debugging][Warning] Code signature found.", file=sys.stderr)

        f.seek(pos)
        f.seek(size, 1)
    return lc_linkedit, lc_symtab


def load_debug_symbols(f: BinaryIO, offset: int, n_symbol: int) -> list[Symbol]:
    """
    // Each LC_SYMTAB entry consists of the following fields:
    // - String Index: 4 bytes (offset into the string table)
    // - Type: 1 byte
    // - Section: 1 byte
    // - Description: 2 bytes
    // - Value: 8 bytes on 64bit, 4 bytes on 32bit
    """
    f.seek(offset)
    data = f.read(n_symbol * _LC_SYMTAB_STRUCT.size)
    # N_OSO entries are a tiny fraction of the symbols, so find them by searching the
    # type bytes (offset 4 of each entry) rather than unpacking every entry.
    types = data[4 :: _LC_SYMTAB_STRUCT.size]
    symbols = []
    index = types.find(N_OSO)
    while index != -1:
        symbols.append(
            Symbol(*_LC_SYMTAB_STRUCT.unpack_from(data, index * _LC_SYMTAB_STRUCT.size))
        )
        index = types.find(N_OSO, index + 1)
    return symbols
