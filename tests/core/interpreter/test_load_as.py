# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import ast
from typing import Any

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.asserts import expect_failure
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


def extract_test_output(stderr: str) -> dict[str, Any]:
    """Extract the DATA_LOAD_TEST_OUTPUT dict from buck2 stderr."""
    marker = "DATA_LOAD_TEST_OUTPUT: "
    for line in stderr.splitlines():
        idx = line.find(marker)
        if idx != -1:
            return ast.literal_eval(line[idx + len(marker) :])  # type: ignore[no-any-return]
    raise AssertionError(f"Could not find '{marker}' in stderr:\n{stderr}")


class LoadAsTest(BuckTestCase):
    @buck_test(data_dir="format")
    async def test_load_as_selects_the_parser(self, buck: Buck) -> None:
        """The hint, not the extension, picks the parser: `deps.lock` is TOML and
        `metadata.txt` is JSON."""
        result = await buck.targets("root//:")
        data = extract_test_output(result.stderr)

        assert data["lock"]["version"] == 3
        assert [p["name"] for p in data["lock"]["package"]] == ["serde", "toml"]
        assert data["metadata"] == {"name": "example", "version": 1}

    @buck_test(data_dir="format")
    async def test_load_as_files_are_tracked_dependencies(self, buck: Buck) -> None:
        """The point of carrying the format on `ImportPath` is that the loaded file is
        a dependency of the parse, so editing it invalidates the build file."""
        await buck.targets("root//:")

        (buck.cwd / "deps.lock").write_text(
            'version = 4\n\n[[package]]\nname = "anyhow"\n'
        )
        (buck.cwd / "metadata.txt").write_text('{"name": "renamed", "version": 2}\n')

        result = await buck.targets("root//:")
        data = extract_test_output(result.stderr)

        assert data["lock"]["version"] == 4
        assert [p["name"] for p in data["lock"]["package"]] == ["anyhow"]
        assert data["metadata"] == {"name": "renamed", "version": 2}

    @buck_test(data_dir="format")
    async def test_load_as_parse_error_names_the_format(self, buck: Buck) -> None:
        """An unparseable file fails the whole package at load time. Because the
        extension no longer implies the parser, the error has to say which one ran
        and where that choice came from."""
        (buck.cwd / "deps.lock").write_text("this is not valid toml {{{\n")

        await expect_failure(
            buck.targets("root//:"),
            stderr_regex=r"Parsing `root//deps\.lock` as toml, as requested by `\?as=toml`",
        )

    @buck_test(data_dir="redundant")
    async def test_parse_error_names_the_format_without_a_hint(
        self,
        buck: Buck,
    ) -> None:
        """The extension is the reader's only clue as to which parser ran, and it is
        an easy one to misread, so name the parser even when nothing overrode it."""
        (buck.cwd / "data.toml").write_text("this is not valid toml {{{\n")

        await expect_failure(
            buck.targets("root//:"),
            # Nothing overrode the format, so nothing follows the parser name.
            stderr_regex=r"Parsing `root//data\.toml` as toml(?!,)",
        )

    @buck_test(data_dir="format")
    async def test_load_as_rejects_an_unknown_format(self, buck: Buck) -> None:
        """An unrecognised hint must be reported as such, rather than being left on
        the path to resurface as a confusing complaint about the path itself."""
        (buck.cwd / "TARGETS.fixture").write_text(
            'load(":metadata.txt?as=zorblax", "value")\n'
        )

        await expect_failure(
            buck.targets("root//:"),
            stderr_regex=r"[Uu]nknown import format `zorblax`",
        )

    @buck_test(data_dir="format")
    async def test_load_as_bzl_is_rejected(self, buck: Buck) -> None:
        """`?as=` names a format the extension cannot. Starlark files can already say
        so with `.bzl`, and accepting `?as=bzl` would make any extension -- including
        `.bxl`, which `load()` may not reach -- evaluable as code."""
        (buck.cwd / "TARGETS.fixture").write_text(
            'load(":deps.lock?as=bzl", "value")\n'
        )

        await expect_failure(
            buck.targets("root//:"),
            stderr_regex=r"[Ii]mport format `bzl` cannot be requested with `\?as=`",
        )

    @buck_test(data_dir="format")
    async def test_load_without_a_hint_still_requires_a_known_extension(
        self,
        buck: Buck,
    ) -> None:
        (buck.cwd / "TARGETS.fixture").write_text('load(":metadata.txt", "value")\n')

        await expect_failure(
            buck.targets("root//:"),
            stderr_regex=r"must have suffix `\.bzl`, `\.json`, or `\.toml`",
        )
