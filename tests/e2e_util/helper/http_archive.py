# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import gzip
import hashlib
import io
import tarfile
import uuid
import zipfile
from collections.abc import Mapping

from buck2.tests.e2e_util.helper.http_server import sha1_hex


def make_archive(extension: str, files: Mapping[str, bytes]) -> bytes:
    """Package archive-relative file paths and their contents without external storage.

    A comment changes the digest of nonempty archives without changing extracted
    files, so a previous CAS entry cannot satisfy a test intended to exercise HTTP.
    """
    comment = uuid.uuid4().hex
    output = io.BytesIO()
    if extension == "zip":
        with zipfile.ZipFile(output, "w") as archive:
            archive.comment = comment.encode()
            for name, content in files.items():
                archive.writestr(name, content)
    else:
        assert extension in ("tar", "tgz"), extension
        with tarfile.open(
            fileobj=output,
            mode="w",
            pax_headers={"comment": comment} if files else None,
        ) as archive:
            for name, content in files.items():
                info = tarfile.TarInfo(name)
                info.size = len(content)
                info.mode = 0o644
                archive.addfile(info, io.BytesIO(content))
    content = output.getvalue()
    return gzip.compress(content, mtime=0) if extension == "tgz" else content


def archive_configs(base_url: str, archives: Mapping[str, bytes]) -> list[str]:
    """Configure the in-repo HTTP archive and Rust fixture targets."""
    args = ["-c", f"test.archive_base_url={base_url}"]
    for extension, content in archives.items():
        args += [
            "-c",
            f"test.archive_{extension}_sha1={sha1_hex(content)}",
            "-c",
            f"test.archive_{extension}_sha256={hashlib.sha256(content).hexdigest()}",
        ]
    return args
