# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import argparse
import configparser
import contextlib
import csv
import io
import json
import os
import re
import shutil
import sys
import zipfile
from types import TracebackType
from typing import cast


def normalize_name(name: str) -> str:
    """
    Per:
      * https://packaging.python.org/en/latest/specifications/recording-installed-packages/#the-dist-info-directory
      * https://packaging.python.org/en/latest/specifications/name-normalization/
    Normalize package names according to PEP503 then replace "-" with "_" as to not conflict with the "-" separating
    the name of the package with the version specification.

    Example: "torch-nightly" normalizes to "torch_nightly.dist-info/" and "torch_nightly.data/"
    """
    pep503_normalized_name = re.sub(r"[-_.]+", "-", name).lower()
    return pep503_normalized_name.replace("-", "_")


def readme_content_type(path: str) -> str:
    _, ext = os.path.splitext(path.lower())
    if ext in (".md", ".markdown"):
        return "text/markdown"
    if ext == ".rst":
        return "text/x-rst"
    return "text/plain"


# pyre-fixme[24]: Generic type `AbstractContextManager` expects 1 type parameter.
class WheelBuilder(contextlib.AbstractContextManager):
    def __init__(
        self,
        *,
        name: str,
        version: str,
        output: str,
        python_tag: str = "py3",
        abi_tag: str = "none",
        platform_tag: str = "any",
        entry_points: dict[str, str] | None = None,
        metadata: list[tuple[str, str]] | None = None,
        readme: str | None = None,
    ) -> None:
        self._name = name

        self._normalized_name: str = normalize_name(name)

        # TODO normalize version like we normalized name above
        #  can follow pypi/packaging.utils.canonicalize_version (see: https://fburl.com/code/amuvl3d2)
        #  punted for later since it was not a clean copy/paste and
        #  taking a dep to tp from toolchains is not straightforward
        self._version = version
        self._python_tag = python_tag
        self._abi_tag = abi_tag
        self._platform_tag = platform_tag
        self._record: list[str] = []
        self._outf = zipfile.ZipFile(output, mode="w")
        self._entry_points: dict[str, str] | None = entry_points
        self._metadata: list[tuple[str, str]] = []
        self._readme = readme
        self._metadata.append(("Name", name))
        self._metadata.append(("Version", version))
        if readme is not None:
            self._metadata.append(
                ("Description-Content-Type", readme_content_type(readme))
            )
        if metadata is not None:
            self._metadata.extend(metadata)

    def _dist_info(self, *path: str) -> str:
        return os.path.join(f"{self._normalized_name}-{self._version}.dist-info", *path)

    def _data(self, *path: str) -> str:
        return os.path.join(f"{self._normalized_name}-{self._version}.data", *path)

    def write(self, dst: str, src: str) -> None:
        if os.path.isdir(src):
            for root, _, files in os.walk(src):
                for file in files:
                    file_src = os.path.join(root, file)
                    file_dst = os.path.join(dst, os.path.relpath(file_src, src))
                    self.write(file_dst, file_src)
        else:
            self._record.append(dst)
            zinfo = zipfile.ZipInfo.from_file(
                filename=src,
                arcname=dst,
                # Allow older timestamps, as we're gonna overwrite them below anyway.
                strict_timestamps=False,
            )
            zinfo.date_time = (1980, 1, 1, 0, 0, 0)
            with open(src, "rb") as fsrc, self._outf.open(zinfo, "w") as fdst:
                shutil.copyfileobj(fsrc, fdst, 1024 * 8)

    def write_data(self, dst: str, src: str) -> None:
        self.write(self._data(dst), src)

    def writestr(self, dst: str, contents: str) -> None:
        self._record.append(dst)
        self._outf.writestr(
            zinfo_or_arcname=zipfile.ZipInfo(filename=dst),
            data=contents,
        )

    def _write_record(self) -> None:
        record = self._dist_info("RECORD")
        # RECORD is CSV, so a path containing a comma has to be quoted or it
        # parses as extra fields and strict installers reject the wheel. For
        # paths needing no quoting csv.writer emits exactly `path,,`.
        buf = io.StringIO()
        writer = csv.writer(buf, lineterminator="\n")
        for path in self._record + [record]:
            writer.writerow([path, "", ""])
        self._outf.writestr(
            zinfo_or_arcname=zipfile.ZipInfo(filename=record),
            data=buf.getvalue(),
        )

    def close(self) -> None:
        metadata = "".join([f"{key}: {val}\n" for key, val in self._metadata])
        if self._readme is not None:
            with open(self._readme, encoding="utf-8") as readme:
                metadata += "\n" + readme.read()

        self.writestr(
            self._dist_info("METADATA"),
            metadata,
        )

        # Determine Root-Is-Purelib based on platform tag
        # Pure Python wheels use platform "any", platform-specific wheels are not purelib
        root_is_purelib = "true" if self._platform_tag == "any" else "false"

        self.writestr(
            self._dist_info("WHEEL"),
            f"""\
Wheel-Version: 1.0
Generator: buck2
Root-Is-Purelib: {root_is_purelib}
Tag: {self._python_tag}-{self._abi_tag}-{self._platform_tag}
""",
        )

        # Write entry points.
        if self._entry_points is not None:
            config = configparser.ConfigParser()
            config.read_dict(cast(dict[str, dict[str, str]], self._entry_points))
            with io.TextIOWrapper(
                self._outf.open(self._dist_info("entry_points.txt"), mode="w"),
                encoding="utf-8",
            ) as f:
                config.write(f)

        self._write_record()
        self._outf.close()

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        exc_tb: TracebackType | None,
    ) -> None:
        self.close()


def main(argv: list[str]) -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--python-tag", default="py3")
    parser.add_argument("--abi-tag", default="none")
    parser.add_argument("--platform-tag", default="any")
    parser.add_argument("--entry-points", default=None)
    parser.add_argument("--manifest", dest="manifests", action="append", default=[])
    parser.add_argument(
        "--src-path", nargs=2, dest="src_paths", action="append", default=[]
    )
    parser.add_argument("--metadata", nargs=2, action="append", default=[])
    parser.add_argument("--readme", default=None)
    parser.add_argument("--data", nargs=2, action="append", default=[])
    args = parser.parse_args(argv[1:])

    pkgs: set[str] = set()
    pkgs_with_init = set()

    def _add_pkg(pkg: str) -> None:
        # root (of 'site-packages/') should not be considered a pkg
        if not pkg:
            return

        pkgs.add(pkg)
        parent = os.path.dirname(pkg)
        _add_pkg(parent)

    with WheelBuilder(
        name=args.name,
        version=args.version,
        output=args.output,
        python_tag=args.python_tag,
        abi_tag=args.abi_tag,
        platform_tag=args.platform_tag,
        entry_points=(
            json.loads(args.entry_points) if args.entry_points is not None else None
        ),
        metadata=args.metadata,
        readme=args.readme,
    ) as whl:
        all_srcs = {}
        for src in args.manifests:
            with open(src) as f:
                manifest = json.load(f)
            for dst, src, *_ in manifest:
                all_srcs[dst] = src
        for dst, src in args.src_paths:
            all_srcs[dst] = src

        for dst, src in sorted(all_srcs.items()):
            if dst.endswith((".py", ".so")):
                pkg = os.path.dirname(dst)
                _add_pkg(pkg)
                if os.path.basename(dst) == "__init__.py":
                    pkgs_with_init.add(pkg)
            whl.write(dst, src)

        for pkg in sorted(pkgs - pkgs_with_init):
            whl.writestr(os.path.join(pkg, "__init__.py"), "")

        for dst, src in args.data:
            whl.write_data(dst, src)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
