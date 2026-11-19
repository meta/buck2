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


class NoRedundantInplaceFalse(LintRule):
    """Flags a redundant `inplace=False` on `@buck_test(...)` in core tests.

    Core tests default to `inplace=False`, so passing it explicitly is
    redundant. Scoped to fbcode/buck2/tests/core via fixit.toml. The autofix
    removes the argument (and its comma).
    """

    ONCALL = "build_infra"
    MESSAGE = "Explicit inplace=False is not needed in core tests; remove it."

    VALID = [
        ValidTestCase(
            """
            @buck_test()
            def test_foo(buck):
                pass
            """
        ),
        ValidTestCase(
            """
            @buck_test(data_dir="pass")
            def test_foo(buck):
                pass
            """
        ),
        # inplace=True is meaningful and must not be touched.
        ValidTestCase(
            """
            @buck_test(inplace=True)
            def test_foo(buck):
                pass
            """
        ),
        # A different call with inplace=False must not be flagged.
        ValidTestCase(
            """
            def f(df):
                return df.drop(inplace=False)
            """
        ),
    ]

    INVALID = [
        InvalidTestCase(
            """
            @buck_test(inplace=False)
            def test_foo(buck):
                pass
            """,
            expected_replacement="""
            @buck_test()
            def test_foo(buck):
                pass
            """,
        ),
        InvalidTestCase(
            """
            @buck_test(inplace=False, data_dir="pass")
            def test_foo(buck):
                pass
            """,
            expected_replacement="""
            @buck_test(data_dir="pass")
            def test_foo(buck):
                pass
            """,
        ),
        InvalidTestCase(
            """
            @buck_test(data_dir="pass", inplace=False)
            def test_foo(buck):
                pass
            """,
            expected_replacement="""
            @buck_test(data_dir="pass")
            def test_foo(buck):
                pass
            """,
        ),
    ]

    def visit_Call(self, node: cst.Call) -> None:
        if not m.matches(node, m.Call(func=m.Name("buck_test"))):
            return
        new_args = [
            arg
            for arg in node.args
            if not (
                arg.keyword is not None
                and arg.keyword.value == "inplace"
                and isinstance(arg.value, cst.Name)
                and arg.value.value == "False"
            )
        ]
        if len(new_args) == len(node.args):
            return
        if new_args:
            # Drop any trailing comma left on the new last argument.
            new_args[-1] = new_args[-1].with_changes(comma=cst.MaybeSentinel.DEFAULT)
        self.report(node, replacement=node.with_changes(args=new_args))
