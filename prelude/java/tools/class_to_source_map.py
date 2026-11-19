# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import os
import shutil
import stat
import zipfile
from collections.abc import Callable, Iterable

# Entries get a fixed date and mode, so the jar depends only on the sources' contents.
_FIXED_DATE_TIME = (1985, 2, 1, 0, 0, 0)
_FILE_MODE = stat.S_IFREG | 0o644


def _source_stem(source_path: str) -> str:
    return os.path.splitext(os.path.basename(source_path))[0]


def _normalize_kotlin_file_facade(class_name: str, source_path: str | None) -> str:
    if source_path is None or not class_name.endswith("Kt"):
        return class_name

    # A Kotlin file facade (`Foo.kt` -> class `FooKt`) is renamed back to the file
    # name, but a real class literally named `FooKt` (from `FooKt.kt`) is left alone.
    simple_name = class_name.rsplit(".", 1)[-1]
    if simple_name[:-2] == _source_stem(source_path):
        return class_name[:-2]
    return class_name


def _source_identity(class_name: str, source_path: str) -> str:
    package, _, _ = class_name.rpartition(".")
    source_stem = _source_stem(source_path)
    return package + "." + source_stem if package else source_stem


def _jar_class_names(jar: str) -> list[str]:
    with zipfile.ZipFile(jar) as jar_file:
        return [
            entry[:-6].replace("/", ".")
            for entry in jar_file.namelist()
            if entry.endswith(".class")
        ]


def _add_source_identity_aliases(entries: dict[str, dict[str, str]]) -> None:
    # Android aggregate coverage identifies a source by package and file stem.
    # Preserve a real class mapping if it has the same name as that identity.
    for entry in list(entries.values()):
        source_path = entry.get("srcPath")
        if source_path is None:
            continue
        identity = _source_identity(entry["className"], source_path)
        entries.setdefault(identity, {"className": identity, "srcPath": source_path})


def write_sources_jar(classes: Iterable[dict[str, str]], output: str) -> None:
    # Several classes can share one source file, so entries are keyed by source path
    # and named after the file rather than the class to avoid source duplication.
    written_sources: set[str] = set()
    with zipfile.ZipFile(output, "w") as sources_jar:
        for entry in classes:
            source_path = entry.get("srcPath")
            if source_path is None or source_path in written_sources:
                continue
            written_sources.add(source_path)
            package, _, _ = entry["className"].rpartition(".")
            package_dir = package.replace(".", "/")
            source_name = os.path.basename(source_path)
            archive_path = (
                package_dir + "/" + source_name if package_dir else source_name
            )
            entry_info = zipfile.ZipInfo(archive_path, date_time=_FIXED_DATE_TIME)
            entry_info.compress_type = sources_jar.compression
            entry_info.create_system = 3  # Unix, for the mode in external_attr.
            entry_info.external_attr = _FILE_MODE << 16
            with open(source_path, "rb") as source:
                entry_info.file_size = os.fstat(source.fileno()).st_size
                with sources_jar.open(entry_info, "w") as dest:
                    shutil.copyfileobj(source, dest, length=8 * 1024)


def build_class_entries(
    jar: str,
    source_for_class: Callable[[str, str], str | None],
    include_classes_prefixes: Iterable[str],
) -> list[dict[str, str]]:
    include_prefixes = tuple(include_classes_prefixes)
    entries: dict[str, dict[str, str]] = {}

    for compiled_class in _jar_class_names(jar):
        top_level_class = compiled_class.split("$", 1)[0]
        source_path = source_for_class(compiled_class, top_level_class)
        class_name = _normalize_kotlin_file_facade(top_level_class, source_path)
        existing_entry = entries.get(class_name)
        if source_path is not None:
            # Nested and synthetic classes collapse to their top-level class. Prefer
            # a sourceful mapping regardless of their order in the JAR.
            if existing_entry is None or "srcPath" not in existing_entry:
                entries[class_name] = {
                    "className": class_name,
                    "srcPath": source_path,
                }
        elif (
            existing_entry is None
            and "$" not in compiled_class
            and compiled_class.startswith(include_prefixes)
        ):
            entries[class_name] = {"className": class_name}

    _add_source_identity_aliases(entries)

    return list(entries.values())
