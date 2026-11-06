# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

"""Runs prelude tools the way the rules run them: as actions of a buck2 build, through the
tools' command lines, in a workspace that uses the bundled prelude."""

import os
from pathlib import Path

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase
from buck2.tests.e2e_util.helper.utils import read_what_ran


async def _output(buck: Buck, target: str) -> Path:
    res = await buck.build_without_report(target, "--show-full-simple-output")
    return Path(res.stdout.strip())


# The compile of `r.c`, built with the real clang the test target names in `CLANG` (clang
# from `PATH` when it is unset). It runs locally: the compiler is a path on this host.
DEP_FILES_LIB = "root//:lib"
# The test writes this header: a checked-in file name may not contain a space.
SPACED_HEADER = "sub/my dir/s.h"
DEP_FILES_HEADERS = [".gen/g.h", SPACED_HEADER, "v.h"]


def _write_spaced_header(buck: Buck) -> None:
    path = buck.cwd / SPACED_HEADER
    path.parent.mkdir(parents=True)
    path.write_text("#define S 1\n")


def _clang_config() -> list[str]:
    clang = os.environ.get("CLANG")
    return ["-c", f"test.clang={clang}"] if clang else []


async def _build_object(buck: Buck) -> None:
    await buck.build(
        f"{DEP_FILES_LIB}[objects][r.c.o]", "--local-only", *_clang_config()
    )


async def _tagged_headers(buck: Buck) -> list[str]:
    """The headers buck2 tracks for the compile, from the dep file the processor wrote."""
    res = await buck.audit_dep_files(
        DEP_FILES_LIB, "c_compile", "r.c", *_clang_config()
    )
    return sorted(
        line.split("\t", 1)[1]
        for line in res.stdout.splitlines()
        if line.startswith("headers\t")
    )


async def _compile_executors(buck: Buck) -> list[str]:
    """How the last command ran the compile of `r.c`: `Local`, or `LocalDepFileCache` when the
    dep file said that no used input changed and the old object was kept."""
    return [
        entry["reproducer"]["executor"]
        for entry in await read_what_ran(buck)
        if "(c_compile r.c)" in entry["identity"]
    ]


class PreludeToolsTest(BuckTestCase):
    # prelude/python/tools/make_py_package_manifest_module.py
    @buck_test(inplace=False)
    async def test_manifest_module_lists_import_names(self, buck: Buck) -> None:
        namespace: dict[str, object] = {}
        exec((await _output(buck, "root//:manifest_module")).read_text(), namespace)
        assert namespace["modules"] == ["pkg.__init__", "pkg._speedups"]

    # prelude/cxx/tools/show_headers_to_dep_file.py
    @buck_test(inplace=False, skip_for_os=["darwin", "windows"])
    async def test_show_headers_dep_file_drops_the_headers_found_next_to_the_source(
        self, buck: Buck
    ) -> None:
        _write_spaced_header(buck)
        await _build_object(buck)
        # The rules compile `./r.c`, so clang prints every header it found relative to the
        # source as `. ./<path>`. The processor turns `./v.h` into `/v.h`, outside the build
        # root, and drops it; the other two paths go the same way.
        assert await _tagged_headers(buck) == []

        # So an edit to the three headers does not rebuild the object.
        for header in DEP_FILES_HEADERS:
            path = buck.cwd / header
            path.write_text(path.read_text().replace(" 1\n", " 2\n"))
        await _build_object(buck)
        assert await _compile_executors(buck) == ["LocalDepFileCache"]
