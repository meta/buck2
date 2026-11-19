# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

from __future__ import annotations

import importlib.resources
import unittest

# Resource name as declared in BUCK -> repo path, used in failure messages.
LOCKFILES: dict[str, str] = {
    "website.yarn.lock": "fbcode/buck2/website/yarn.lock",
    "explain_js.yarn.lock": "fbcode/buck2/app/buck2_explain/js/yarn.lock",
    "explain_output_format_js.yarn.lock": (
        "fbcode/buck2/app/buck2_explain/output_format_js/yarn.lock"
    ),
}

REMEDIATION = (
    "`npm install` rewrites a co-located `yarn.lock` into npm's own format, "
    "converting every entry and reporting nothing. Regenerate this lockfile with "
    "yarn in a scratch directory that npm has not run in, rather than editing it. "
    "See fbcode/buck2/docs/developers/js_dependencies.md"
)


class YarnLockfileTest(unittest.TestCase):
    def test_resolves_through_yarn_registry(self) -> None:
        """No `yarn.lock` should reference `registry.npmjs.org`.

        yarn resolves through `registry.yarnpkg.com`; npmjs.org URLs mean npm has
        rewritten the file.
        """
        for resource, path in LOCKFILES.items():
            with self.subTest(path=path):
                contents = (
                    importlib.resources.files(__package__)
                    .joinpath(resource)
                    .read_text()
                )
                offenders = [
                    line.strip()
                    for line in contents.splitlines()
                    if "registry.npmjs.org" in line
                ]
                self.assertEqual(
                    [],
                    offenders,
                    f"{path} contains {len(offenders)} registry.npmjs.org "
                    f"reference(s). {REMEDIATION}",
                )
