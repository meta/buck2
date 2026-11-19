#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


import unittest

from buck2.tests.e2e_util.helper.utils import get_targets_from_what_ran

WHAT_RAN = [
    {"identity": "root//:t (cfg#0123456789abcdef) (strip_debug libfoo.so)"},
    {"identity": "anon//:anon_t (strip_debug libfoo.so)"},
]


class WhatRanIdentityTest(unittest.TestCase):
    def test_an_identity_without_a_configuration_keeps_its_category(self) -> None:
        self.assertEqual(
            get_targets_from_what_ran(WHAT_RAN),
            {
                ("root//:t", "strip_debug libfoo.so"),
                ("anon//:anon_t", "strip_debug libfoo.so"),
            },
        )
