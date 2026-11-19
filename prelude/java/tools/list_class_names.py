# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import argparse
import pathlib
import zipfile


def _parse_args():
    parser = argparse.ArgumentParser(
        description="Tool to list all the classes found in a jar."
    )

    parser.add_argument(
        "--jar",
        type=pathlib.Path,
        required=True,
        help="a path to a .jar to list classes for",
    )
    parser.add_argument(
        "--sources",
        type=pathlib.Path,
        required=True,
        help="A file containing a list of sources that were used to create the .jar",
    )
    parser.add_argument(
        "--output",
        type=pathlib.Path,
        required=True,
        help="a path to write the output to",
    )
    parser.add_argument(
        "--discover-all",
        action="store_true",
        default=False,
        help="Include all top-level classes from the jar, skipping stem-based "
        "source matching. Useful for Kotlin targets where file names may not "
        "match class names.",
    )

    return parser.parse_args()


_SUPPORTED_ARCHIVE_SUFFIXES = [".src.zip", "-sources.jar"]


def _is_supported_archive(src):
    for supported_suffix in _SUPPORTED_ARCHIVE_SUFFIXES:
        if src.name.endswith(supported_suffix):
            return True
    return False


def _get_class_names(
    sources_path: pathlib.Path, jar_path: pathlib.Path, discover_all: bool = False
):
    sources = set()
    if not discover_all:
        with open(sources_path, "r") as sources_file:
            for line in sources_file.readlines():
                source_file_path = pathlib.Path(line.strip())
                if _is_supported_archive(source_file_path):
                    with zipfile.ZipFile(source_file_path, "r") as sources_archive:
                        for archived_source in sources_archive.namelist():
                            sources.add(pathlib.Path(archived_source).stem)
                else:
                    sources.add(source_file_path.stem)

    with zipfile.ZipFile(jar_path, "r") as jar:
        class_names = []
        for file_name in jar.namelist():
            if not file_name.endswith(".class"):
                continue

            if "$" in file_name:
                continue

            if discover_all or pathlib.Path(file_name).stem in sources:
                # Remove the .class suffix and convert the path of the
                # compiled class to a fully-qualified name.
                class_names.append(file_name[:-6].replace("/", "."))

    return class_names


def main():
    args = _parse_args()
    sources = args.sources
    jar = args.jar
    output = args.output

    classes = _get_class_names(sources, jar, discover_all=args.discover_all)
    with open(output, "a") as output_file:
        output_file.write("\n".join(classes))


if __name__ == "__main__":
    main()
