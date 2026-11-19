# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import itertools
import unittest

from .assemble_bundle_types import BundleSpecItem


class TestBundleSpecItemOrder(unittest.TestCase):
    def test_items_are_ordered_by_source_then_destination(self) -> None:
        first = BundleSpecItem(src="a/x", dst="Resources/c")
        second = BundleSpecItem(src="b/x", dst="Resources/b")
        self.assertTrue(first < second)
        self.assertFalse(second < first)

        third = BundleSpecItem(src="c/x", dst="Resources/a")
        sorted_sources = {
            tuple(item.src for item in sorted(permutation))
            for permutation in itertools.permutations([first, second, third])
        }
        self.assertEqual(sorted_sources, {("a/x", "b/x", "c/x")})

    def test_optional_fields_sort_none_first(self) -> None:
        plain = BundleSpecItem(src="a", dst="b")
        with_entitlements = BundleSpecItem(src="a", dst="b", codesign_entitlements="e")
        self.assertTrue(plain < with_entitlements)
        self.assertFalse(with_entitlements < plain)
        self.assertFalse(plain < BundleSpecItem(src="a", dst="b"))
