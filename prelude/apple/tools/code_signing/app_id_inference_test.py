# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import unittest

from .app_id import AppId


class TestAppIdInference(unittest.TestCase):
    def test_team_comes_from_the_first_prefixed_keychain_group(self) -> None:
        self.assertEqual(
            AppId.infer_from_entitlements(
                {
                    "keychain-access-groups": [
                        "com.apple.token",
                        "ABCDE12345.com.example.app",
                    ]
                }
            ),
            AppId("ABCDE12345", "com.example.app"),
        )
        # A list without any prefixed group still fails on its first entry.
        with self.assertRaisesRegex(
            RuntimeError, "Malformed app ID string: 'com.apple.token'"
        ):
            AppId.infer_from_entitlements(
                {"keychain-access-groups": ["com.apple.token"]}
            )
