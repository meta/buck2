# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-unsafe

import libcst as cst
import libcst.matchers as m
from fixit import InvalidTestCase, LintRule, ValidTestCase


# The fixit.toml override can only scope to a directory, and `tests/e2e/build/`
# holds many files that legitimately use `inplace=True`. The single-file scope
# the PatternLint original got from its `include` therefore has to be enforced
# here instead.
_TARGET_FILE = "test_build_isolated.py"

# `fixit`'s own test harness lints every VALID/INVALID case under a synthetic
# `valid.py`/`invalid.py` path, so the filename guard has to admit those or the
# INVALID cases below would stop reporting and silently pass.
_TEST_HARNESS_FILES = ("valid.py", "invalid.py")


class NoInplaceTrueInBuildIsolated(LintRule):
    """Flags `@buck_test(inplace=True)` in test_build_isolated.py.

    Isolated build tests must not run inplace; such tests belong in
    test_build_inplace.py.
    """

    METADATA_DEPENDENCIES = (cst.metadata.FilePathProvider,)

    # Fail closed: if the path metadata is ever unavailable, report nothing
    # rather than reporting on every file the override covers.
    _in_target_file: bool = False

    ONCALL = "build_infra"
    MESSAGE = (
        "Do not use inplace=True tests in test_build_isolated; "
        "place this test in test_build_inplace.py instead."
    )

    VALID = [
        ValidTestCase(
            """
            @buck_test(inplace=False)
            def test_foo(buck):
                pass
            """
        ),
        ValidTestCase(
            """
            @buck_test()
            def test_foo(buck):
                pass
            """
        ),
        # A different decorator with inplace=True must not be flagged.
        ValidTestCase(
            """
            @other_decorator(inplace=True)
            def test_foo(buck):
                pass
            """
        ),
    ]

    INVALID = [
        InvalidTestCase(
            """
            @buck_test(inplace=True)
            def test_foo(buck):
                pass
            """
        ),
        InvalidTestCase(
            """
            @buck_test(inplace=True, data_dir="pass")
            def test_foo(buck):
                pass
            """
        ),
    ]

    def visit_Module(self, node: cst.Module) -> None:
        path = self.get_metadata(cst.metadata.FilePathProvider, node, None)
        name = path.name if path is not None else None
        self._in_target_file = name == _TARGET_FILE or name in _TEST_HARNESS_FILES

    def visit_Call(self, node: cst.Call) -> None:
        if not self._in_target_file:
            return
        if not m.matches(node, m.Call(func=m.Name("buck_test"))):
            return
        for arg in node.args:
            if (
                arg.keyword is not None
                and arg.keyword.value == "inplace"
                and isinstance(arg.value, cst.Name)
                and arg.value.value == "True"
            ):
                self.report(node)
                return
