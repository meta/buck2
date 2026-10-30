# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict


import json

from buck2.tests.e2e_util.api.buck import Buck
from buck2.tests.e2e_util.buck_workspace import buck_test, BuckTestCase


class TargetsImportsAsTomlTest(BuckTestCase):
    @buck_test()
    async def test_imports_as_toml(self, buck: Buck) -> None:
        """A `load(":<file>?as=toml", ...)` is reported as a leaf import by
        `targets --streaming --imports`, and is parsed as TOML, even though the
        file's extension (`.lock`) does not imply TOML. The `?as=toml` hint is
        stripped from the reported path. Covers both a nested load (via a `.bzl`)
        and a top-level load directly in the build file."""
        result = await buck.targets("//...", "--json", "--streaming", "--imports")
        xs = json.loads(result.stdout)

        found_targets = False
        found_bzl = False
        found_lock = False
        found_direct = False

        for x in xs:
            if "buck.imports" not in x:
                continue
            file = x["buck.file"]
            imports = x["buck.imports"]

            if file == "root//TARGETS.fixture":
                assert "root//uses_lock.bzl" in imports
                # Top-level `?as=toml` load, reported without the hint.
                assert "root//direct.lock" in imports
                found_targets = True
            elif file == "root//uses_lock.bzl":
                # The `?as=toml` hint is not part of the reported import path.
                assert "root//uv.lock" in imports
                found_bzl = True
            elif file == "root//uv.lock":
                # Parsed as TOML (empty sub-imports), not as a bzl file (which
                # would carry an implicit `prelude//prelude.bzl` import).
                assert imports == []
                found_lock = True
            elif file == "root//direct.lock":
                assert imports == []
                found_direct = True

        assert found_targets, "TARGETS.fixture imports should be reported"
        assert found_bzl, "uses_lock.bzl imports (including uv.lock) should be reported"
        assert found_lock, (
            "nested uv.lock?as=toml should appear as a leaf import with empty sub-imports"
        )
        assert found_direct, (
            "top-level direct.lock?as=toml should appear as a leaf import with empty sub-imports"
        )
