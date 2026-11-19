# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import unittest
from datetime import datetime
from pathlib import Path

from .apple_platform import ApplePlatform
from .identity import CodeSigningIdentity
from .info_plist_metadata import InfoPlistMetadata
from .provisioning_profile_metadata import ProvisioningProfileMetadata
from .provisioning_profile_selection import select_best_provisioning_profile_core

_IDENTITY = CodeSigningIdentity("fingerprint", "name")


def _profile(name: str, app_id: str, year: int) -> ProvisioningProfileMetadata:
    return ProvisioningProfileMetadata(
        Path(f"/profiles/{name}.mobileprovision"),
        "00000000-0000-0000-0000-000000000000",
        datetime(year, 1, 1),
        {"iOS"},
        {_IDENTITY.fingerprint},
        {"application-identifier": app_id},
    )


class TestProvisioningProfileFilter(unittest.TestCase):
    def setUp(self) -> None:
        self.dev = _profile("dev", "ABCDEFGHIJ.com.company.application", 2100)
        self.adhoc = _profile("adhoc", "ABCDEFGHIJ.com.company.application", 2099)
        self.wildcard = _profile("wildcard", "ABCDEFGHIJ.*", 2100)

    def select(self, profiles, profile_filter):
        return select_best_provisioning_profile_core(
            info_plist_metadata=InfoPlistMetadata(
                "com.company.application", None, False
            ),
            code_signing_identities=[_IDENTITY],
            provisioning_profiles=profiles,
            entitlements={},
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=profile_filter,
        )

    def test_filter_matching_one_best_profile_selects_it(self):
        selected, _ = self.select([self.dev, self.adhoc], "adhoc")
        self.assertEqual(selected.profile, self.adhoc)

    def test_filter_matching_no_best_profile_selects_nothing_without_diagnostics(self):
        # `_select_best_provisioning_profile` in codesign_bundle.py raises an internal
        # RuntimeError on this pair, instead of a CodeSignProvisioningError.
        selected, diagnostics = self.select([self.dev, self.adhoc], "release")
        self.assertIsNone(selected)
        self.assertEqual(diagnostics, [])

    def test_filter_matching_only_a_wildcard_profile_selects_nothing(self):
        selected, diagnostics = self.select(
            [self.dev, self.adhoc, self.wildcard], "wildcard"
        )
        self.assertIsNone(selected)
        self.assertEqual(diagnostics, [])
