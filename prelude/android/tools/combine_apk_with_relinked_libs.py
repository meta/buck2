# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

"""Combine an APK/AAB with relinked native libraries.

Replaces the un-relinked .so files in the input APK/AAB with their
relinked counterparts. A JSON mapping file specifies which libraries
to replace, keyed by <abi>/<soname> (e.g. "arm64-v8a/libfoo.so").

The mapping is produced by the build system and consumed here, so the
script doesn't need to discover or infer what to replace.

Asset libraries are listed with their size and sha256 in
assets/lib/metadata.txt, and a Voltron module's in assets/<module>/assets/libs.txt
(as assets/<abi>/<soname> or lib/<abi>/<soname>). Those lines are rewritten for
every replaced library, so the output matches what a build that relinks before
packaging produces.

Handles .so files at all possible packaging locations:
  - lib/<abi>/<soname>                        (standard)
  - assets/lib/<abi>/<soname>                 (asset libraries)
  - assets/<module>/lib/<abi>/<soname>        (Voltron module libs)
  - assets/<module>/assets/<abi>/<soname>     (Voltron module assets)
  - Inside nested ZIP/JAR files               (module containers)

Usage:
    combine_apk_with_relinked_libs.py \
        --input input.apk \
        --relinked-libs-dir /path/to/relinked_libs \
        --mapping /path/to/mapping.json \
        --output output.apk
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import shutil
import struct
import zipfile


# A zip local file header: signature, version, flags, method, time, date, crc,
# compressed size, uncompressed size, name length, extra length.
_LOCAL_HEADER = struct.Struct("<4s5H3L2H")


def _without_zip64_extra(extra: bytes) -> bytes:
    """Drops the zip64 extra field, which `ZipInfo.FileHeader` writes itself
    when the sizes need it."""
    out = b""
    i = 0
    while i + 4 <= len(extra):
        tag, size = struct.unpack_from("<HH", extra, i)
        if tag != 0x0001:
            out += extra[i : i + 4 + size]
        i += 4 + size
    return out


def _copy_entry_raw(
    zin: zipfile.ZipFile, zout: zipfile.ZipFile, info: zipfile.ZipInfo
) -> None:
    """Copies an entry's compressed bytes as they are.

    zipfile has no API for this; re-deflating every unchanged entry dominated
    the run time (26 s against 2 s on a 626 MB APK). This writes the local
    header and data through `zout.fp` and registers the entry the way
    `ZipFile.writestr` does, so `close()` writes the central directory.
    """
    zin.fp.seek(info.header_offset)
    header = _LOCAL_HEADER.unpack(zin.fp.read(_LOCAL_HEADER.size))
    zin.fp.seek(header[9] + header[10], os.SEEK_CUR)
    data = zin.fp.read(info.compress_size)

    out = zipfile.ZipInfo(info.filename, info.date_time)
    out.compress_type = info.compress_type
    out.comment = info.comment
    out.extra = _without_zip64_extra(info.extra)
    out.create_system = info.create_system
    out.create_version = info.create_version
    out.extract_version = info.extract_version
    # The sizes go in the local header, so no data descriptor follows.
    out.flag_bits = info.flag_bits & ~0x08
    out.internal_attr = info.internal_attr
    out.external_attr = info.external_attr
    out.CRC = info.CRC
    out.compress_size = info.compress_size
    out.file_size = info.file_size
    out.header_offset = zout.fp.tell()
    zout.fp.write(out.FileHeader())
    zout.fp.write(data)
    zout.filelist.append(out)
    zout.NameToInfo[out.filename] = out
    zout.start_dir = zout.fp.tell()
    zout._didModify = True


def _build_relinked_lookup(
    mapping: list[str],
    relinked_libs_dir: str,
) -> dict[str, str]:
    """Build a lookup dict from the mapping list.

    Args:
        mapping: List of "<abi>/<soname>" strings from the build system.
        relinked_libs_dir: Base directory containing the relinked .so files.

    Returns:
        Dict mapping "<abi>/<soname>" to the absolute filesystem path.
    """
    lookup = {}
    for abi_soname in mapping:
        path = os.path.join(relinked_libs_dir, abi_soname)
        if not os.path.isfile(path):
            raise RuntimeError(
                "Relinked library not found: {} (expected at {})".format(
                    abi_soname, path
                )
            )
        lookup[abi_soname] = path
    return lookup


def _find_matching_key(
    zip_entry: str,
    relinked_lookup: dict[str, str],
) -> str | None:
    """Find the mapping key that matches a ZIP entry path.

    Keys are always <abi>/<soname> (exactly two path components), so we
    extract the last two components of the zip entry and do a direct
    dict lookup — O(1) per entry instead of O(M) linear scan.

    Note: suffix matching is intentional — the same <abi>/<soname> may appear
    at multiple packaging locations (lib/, assets/lib/, assets/<module>/lib/,
    etc.) and all occurrences should be replaced with the same relinked binary.
    """
    parts = zip_entry.rsplit("/", 2)
    if len(parts) >= 2:
        candidate = parts[-2] + "/" + parts[-1]
        if candidate in relinked_lookup:
            return candidate
    return None


_ASSET_LIB_METADATA_NAMES = ("metadata.txt", "libs.txt")


def _is_asset_lib_metadata(zip_entry: str) -> bool:
    return (
        "assets/" in zip_entry
        and zip_entry.rsplit("/", 1)[-1] in _ASSET_LIB_METADATA_NAMES
    )


def _rewrite_asset_lib_metadata(
    content: bytes,
    relinked_lookup: dict[str, str],
) -> bytes | None:
    """Update the size and sha256 of every replaced library in an asset
    library metadata file, keeping native_libs_as_assets_metadata.py's format
    and order (decreasing size, then path) and whether the file ends in a
    newline. Paths are matched on their last two components, like the .so
    entries themselves.

    Returns None, leaving the file as it is, unless every line is a
    `<abi>/<lib> <size> <sha256>` entry and at least one names a replaced
    library: other assets share the metadata.txt name.
    """
    try:
        text = content.decode("utf-8")
    except UnicodeDecodeError:
        return None
    trailing_newline = text.endswith("\n")
    if trailing_newline:
        text = text[:-1]
    entries = []
    replaced_any = False
    for line in text.split("\n"):
        parts = line.split(" ")
        if len(parts) != 3 or not parts[1].isdigit() or len(parts[2]) != 64:
            return None
        path, size, sha256 = parts[0], int(parts[1]), parts[2]
        relinked = relinked_lookup.get("/".join(path.rsplit("/", 2)[-2:]))
        if relinked is not None:
            with open(relinked, "rb") as f:
                data = f.read()
            size, sha256 = len(data), hashlib.sha256(data).hexdigest()
            replaced_any = True
        entries.append((path, size, sha256))
    if not replaced_any:
        return None
    entries.sort(key=lambda e: (-e[1], e[0]))
    rewritten = "\n".join(
        "{} {} {}".format(path, size, sha256) for path, size, sha256 in entries
    )
    return (rewritten + ("\n" if trailing_newline else "")).encode("utf-8")


def _maybe_rebuild_inner_zip(
    inner_bytes: bytes,
    relinked_lookup: dict[str, str],
) -> tuple[bytes, dict[str, set[int]]] | None:
    """Rebuild a nested ZIP, replacing any .so entries with relinked versions.

    Returns a tuple of (rebuilt bytes, the CRCs of the entries replaced under
    each key) if any replacements were made, or None if the inner ZIP has no
    matching entries (or is not a valid ZIP). This avoids parsing the same ZIP
    twice (once to check, once to rebuild).
    """
    try:
        buf = io.BytesIO(inner_bytes)
        with zipfile.ZipFile(buf, "r") as zin:
            has_match = any(
                _find_matching_key(info.filename, relinked_lookup)
                for info in zin.infolist()
            )
            if not has_match:
                return None
            inner_replaced: dict[str, set[int]] = {}
            out = io.BytesIO()
            with zipfile.ZipFile(out, "w") as zout:
                for info in zin.infolist():
                    key = _find_matching_key(info.filename, relinked_lookup)
                    if key is not None:
                        # writestr replaces info.CRC with the relinked library's.
                        inner_replaced.setdefault(key, set()).add(info.CRC)
                        with open(relinked_lookup[key], "rb") as f:
                            zout.writestr(info, f.read())
                    else:
                        data = zin.read(info.filename)
                        if _is_asset_lib_metadata(info.filename):
                            data = (
                                _rewrite_asset_lib_metadata(data, relinked_lookup)
                                or data
                            )
                        zout.writestr(info, data)
            return out.getvalue(), inner_replaced
    except zipfile.BadZipFile:
        return None


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Replace .so files in an APK/AAB with relinked versions."
    )
    parser.add_argument(
        "--input",
        required=True,
        help="Path to the input APK/AAB",
    )
    parser.add_argument(
        "--relinked-libs-dir",
        required=True,
        help="Path to directory containing relinked .so files (structure: <abi>/<soname>)",
    )
    parser.add_argument(
        "--mapping",
        required=True,
        help="Path to JSON file listing <abi>/<soname> entries to replace",
    )
    parser.add_argument(
        "--output",
        required=True,
        help="Path to write the combined output APK/AAB",
    )
    args = parser.parse_args()

    with open(args.mapping) as f:
        mapping = json.load(f)

    relinked_lookup = _build_relinked_lookup(mapping, args.relinked_libs_dir)

    if not relinked_lookup:
        shutil.copy2(args.input, args.output)
        return

    # The CRCs of the entries replaced under each key. The copies of one library
    # are identical; two different entries under one key are two libraries with
    # the same soname, and replacing both would replace one that was not
    # relinked.
    replaced: dict[str, set[int]] = {}
    with zipfile.ZipFile(args.input, "r") as zin:
        with zipfile.ZipFile(args.output, "w") as zout:
            for info in zin.infolist():
                key = _find_matching_key(info.filename, relinked_lookup)
                if key is not None:
                    # writestr replaces info.CRC with the relinked library's.
                    replaced.setdefault(key, set()).add(info.CRC)
                    with open(relinked_lookup[key], "rb") as f:
                        data = f.read()
                    zout.writestr(info, data)
                elif info.filename.endswith((".jar", ".zip")):
                    inner_bytes = zin.read(info.filename)
                    result = _maybe_rebuild_inner_zip(inner_bytes, relinked_lookup)
                    if result is not None:
                        rebuilt, inner_replaced = result
                        zout.writestr(info, rebuilt)
                        for key, crcs in inner_replaced.items():
                            replaced.setdefault(key, set()).update(crcs)
                    else:
                        _copy_entry_raw(zin, zout, info)
                elif _is_asset_lib_metadata(info.filename):
                    data = zin.read(info.filename)
                    zout.writestr(
                        info, _rewrite_asset_lib_metadata(data, relinked_lookup) or data
                    )
                else:
                    _copy_entry_raw(zin, zout, info)

    ambiguous = sorted(key for key, crcs in replaced.items() if len(crcs) > 1)
    if ambiguous:
        raise RuntimeError(
            "Input packages different libraries under the same <abi>/<soname>, "
            "so the relinked library cannot replace only its own copies: {}".format(
                ", ".join(ambiguous)
            )
        )
    not_replaced = set(relinked_lookup.keys()) - replaced.keys()
    if not_replaced:
        raise RuntimeError(
            "Relinked libraries not found in input: {}".format(
                ", ".join(sorted(not_replaced))
            )
        )


if __name__ == "__main__":
    main()
