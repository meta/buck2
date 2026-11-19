# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import libcst as cst
import libcst.matchers as m
from fixit import InvalidTestCase, LintRule, ValidTestCase


_SKIP_NAMES = (
    m.Name("skip") | m.Name("skipIf") | m.Name("skipUnless") | m.Name("SkipTest")
)


class NoUnittestSkip(LintRule):
    """Flags `unittest`'s skip decorators, `skipTest()` and `SkipTest` in e2e tests.

    TestX reports a test that skips on every run of a platform as broken.
    `buck_test(skip_for_os=..., skip_for_materializer=...)` passes the test
    where it does not apply instead.
    """

    ONCALL = "build_infra"
    MESSAGE = (
        "Do not skip e2e tests; use `@buck_test(skip_for_os=[...])` or "
        "`skip_for_materializer=[...]`, which passes the test where it does not apply."
    )

    VALID = [
        ValidTestCase(
            """
            @buck_test(skip_for_os=["darwin", "windows"])
            async def test_foo(self, buck):
                pass
            """
        ),
        ValidTestCase(
            """
            import unittest

            class FooTest(unittest.TestCase):
                pass
            """
        ),
        # The `@oss-enable` form is a comment internally.
        ValidTestCase(
            """
            @unittest.skip("requires a Meta internal build") # @oss-enable
            @buck_test()
            async def test_foo(self, buck):
                pass
            """
        ),
    ]

    INVALID = [
        InvalidTestCase(
            """
            @unittest.skipIf(sys.platform != "linux", reason="Linux only")
            @buck_test()
            async def test_foo(self, buck):
                pass
            """
        ),
        InvalidTestCase(
            """
            @unittest.skip("broken")
            class FooTest(BuckTestCase):
                pass
            """
        ),
        InvalidTestCase(
            """
            from unittest import skipUnless
            """
        ),
        InvalidTestCase(
            """
            async def test_foo(self, buck):
                self.skipTest("not on Eden")
            """
        ),
        InvalidTestCase(
            """
            async def test_foo(self, buck):
                raise unittest.SkipTest("not on Eden")
            """
        ),
    ]

    def visit_Attribute(self, node: cst.Attribute) -> None:
        if m.matches(node, m.Attribute(value=m.Name("unittest"), attr=_SKIP_NAMES)):
            self.report(node)

    def visit_ImportFrom(self, node: cst.ImportFrom) -> None:
        if not m.matches(node, m.ImportFrom(module=m.Name("unittest"))):
            return
        if isinstance(node.names, cst.ImportStar) or any(
            m.matches(alias.name, _SKIP_NAMES) for alias in node.names
        ):
            self.report(node)

    def visit_Call(self, node: cst.Call) -> None:
        if m.matches(node, m.Call(func=m.Attribute(attr=m.Name("skipTest")))):
            self.report(node)
