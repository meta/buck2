# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import copy
import unittest
from datetime import datetime, timedelta
from pathlib import Path

from .apple_platform import ApplePlatform
from .identity import CodeSigningIdentity
from .info_plist_metadata import InfoPlistMetadata
from .provisioning_profile_diagnostics import IProvisioningProfileDiagnostics
from .provisioning_profile_metadata import ProvisioningProfileMetadata
from .provisioning_profile_selection import (
    _matches_or_array_is_subset_of,
    select_best_provisioning_profile_core,
    SelectedProvisioningProfileInfo,
)


class TestSelection(unittest.TestCase):
    def verify_diagnostic_info_candidate_profile(
        self,
        diagnostic_info: list[IProvisioningProfileDiagnostics],
        reason: str,
    ):
        self.assertEqual(len(diagnostic_info), 1)
        candidate_profile = diagnostic_info[0]
        self.assertEqual(
            candidate_profile.log_message(),
            reason,
        )

    def test_expired_profiles_are_ignored(self):
        info_plist = InfoPlistMetadata("com.company.application", None, False)
        identity = CodeSigningIdentity(
            "fingerprint",
            "name",
        )
        expired_provisioning_profile = ProvisioningProfileMetadata(
            Path("/foo"),
            "00000000-0000-0000-0000-000000000000",
            datetime.min,
            {"iOS"},
            {identity.fingerprint},
            {"application-identifier": "ABCDEFGHIJ.com.company.application"},
        )
        selected, diagnostic_info = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[identity],
            provisioning_profiles=[expired_provisioning_profile],
            entitlements={},
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=None,
        )
        self.assertIsNone(selected)
        self.verify_diagnostic_info_candidate_profile(
            diagnostic_info,
            "Provisioning profile expired.",
        )

        fresh_provisioning_profiles = copy.copy(expired_provisioning_profile)
        fresh_provisioning_profiles.expiration_date = datetime.max
        selected, _ = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[identity],
            provisioning_profiles=[fresh_provisioning_profiles],
            entitlements={},
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=None,
        )
        self.assertIsNotNone(selected)

    def test_multiple_matching_profiles_strict_mode(self):
        info_plist = InfoPlistMetadata("com.company.application", None, False)
        identity = CodeSigningIdentity(
            "fingerprint",
            "name",
        )
        first = ProvisioningProfileMetadata(
            Path("/foo.first"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max,
            {"iOS"},
            {identity.fingerprint},
            {"application-identifier": "AAAAAAAAAA.*"},
        )
        second = ProvisioningProfileMetadata(
            Path("/foo.second"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max,
            {"iOS"},
            {identity.fingerprint},
            {"application-identifier": "AAAAAAAAAA.*"},
        )
        profiles = [
            first,
            second,
        ]

        selection_failed = False
        try:
            _, _ = select_best_provisioning_profile_core(
                info_plist_metadata=info_plist,
                code_signing_identities=[identity],
                provisioning_profiles=profiles,
                entitlements={"keychain-access-groups": ["AAAAAAAAAA.*"]},
                platform=ApplePlatform.ios_device,
                strict_search=True,
                provisioning_profile_filter=None,
            )
        except Exception:
            selection_failed = True

        self.assertTrue(selection_failed)

    def test_multiple_matching_profiles_with_preference(self):
        info_plist = InfoPlistMetadata("com.company.application", None, False)
        identity = CodeSigningIdentity(
            "fingerprint",
            "name",
        )
        first = ProvisioningProfileMetadata(
            Path("/foo.first"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max,
            {"iOS"},
            {identity.fingerprint},
            {"application-identifier": "AAAAAAAAAA.*"},
        )
        second = ProvisioningProfileMetadata(
            Path("/foo.second"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max,
            {"iOS"},
            {identity.fingerprint},
            {"application-identifier": "AAAAAAAAAA.*"},
        )
        third = ProvisioningProfileMetadata(
            Path("/foo.third"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max,
            {"iOS"},
            {identity.fingerprint},
            {"application-identifier": "AAAAAAAAAA.*"},
        )
        profiles = [
            first,
            second,
            third,
        ]

        selection_failed = False
        try:
            _, _ = select_best_provisioning_profile_core(
                info_plist_metadata=info_plist,
                code_signing_identities=[identity],
                provisioning_profiles=profiles,
                entitlements={"keychain-access-groups": ["AAAAAAAAAA.*"]},
                platform=ApplePlatform.ios_device,
                strict_search=True,
                provisioning_profile_filter=None,
            )
        except Exception:
            selection_failed = True

        # Check selection fails without preference
        self.assertTrue(selection_failed)

        selected_profile_info, _ = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[identity],
            provisioning_profiles=profiles,
            entitlements={"keychain-access-groups": ["AAAAAAAAAA.*"]},
            platform=ApplePlatform.ios_device,
            strict_search=True,
            provisioning_profile_filter=".+second",
        )
        # Check the middle profile got chosen (i.e., not first or last)
        self.assertEqual(selected_profile_info.profile, second)

    def test_multiple_matching_profiles_nonstrict_mode(self):
        info_plist = InfoPlistMetadata("com.company.application", None, False)
        identity = CodeSigningIdentity(
            "fingerprint",
            "name",
        )
        first = ProvisioningProfileMetadata(
            Path("/foo.first"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max - timedelta(days=2),
            {"iOS"},
            {identity.fingerprint},
            {"application-identifier": "AAAAAAAAAA.*"},
        )
        second = ProvisioningProfileMetadata(
            Path("/foo.second"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max - timedelta(days=1),
            {"iOS"},
            {identity.fingerprint},
            {"application-identifier": "AAAAAAAAAA.*"},
        )
        third = ProvisioningProfileMetadata(
            Path("/foo.third"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max - timedelta(days=3),
            {"iOS"},
            {identity.fingerprint},
            {"application-identifier": "AAAAAAAAAA.*"},
        )
        profiles = [
            first,
            second,
            third,
        ]

        selected_profile_info, _ = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[identity],
            provisioning_profiles=profiles,
            entitlements={"keychain-access-groups": ["AAAAAAAAAA.*"]},
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=None,
        )

        # Check that we selected the profile with the latest expiration date
        # given multiple matches in non-strict mode
        self.assertEqual(selected_profile_info.profile, second)

    def test_prefix_override(self):
        info_plist = InfoPlistMetadata("com.company.application", None, False)
        identity = CodeSigningIdentity(
            "fingerprint",
            "name",
        )
        expected = ProvisioningProfileMetadata(
            Path("/foo"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max,
            {"iOS"},
            {identity.fingerprint},
            {"application-identifier": "AAAAAAAAAA.*"},
        )
        profiles = [
            expected,
            ProvisioningProfileMetadata(
                Path("/foo"),
                "00000000-0000-0000-0000-000000000000",
                datetime.max,
                {"iOS"},
                {identity.fingerprint},
                {"application-identifier": "BBBBBBBBBB.com.company.application"},
            ),
        ]
        selected, _ = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[identity],
            provisioning_profiles=profiles,
            entitlements={"keychain-access-groups": ["AAAAAAAAAA.*"]},
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=None,
        )
        self.assertEqual(selected, SelectedProvisioningProfileInfo(expected, identity))

    def test_entitlement_keys_are_matched(self):
        info_plist = InfoPlistMetadata("com.company.application", None, False)
        identity = CodeSigningIdentity(
            "fingerprint",
            "name",
        )
        expected = ProvisioningProfileMetadata(
            Path("/foo"),
            "11111111-1111-1111-1111-111111111111",
            datetime.max,
            {"iOS"},
            {identity.fingerprint},
            {
                "application-identifier": "AAAAAAAAAA.com.company.application",
                "keychain-access-groups": ["AAAAAAAAAA.*"],
                "aps-environment": "production",
                "com.apple.security.application-groups": ["foo", "bar", "baz"],
                "com.apple.developer.devicecheck.appattest-environment": [
                    "development",
                    "production",
                ],
            },
        )
        unexpected = ProvisioningProfileMetadata(
            Path("/foo"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max,
            {"iOS"},
            {identity.fingerprint},
            {
                "application-identifier": "AAAAAAAAAA.com.company.application",
                "keychain-access-groups": ["AAAAAAAAAA.*"],
                "aps-environment": "development",
                "com.apple.security.application-groups": ["foo", "bar"],
            },
        )
        profiles = [
            expected,
            unexpected,
        ]
        selected, _ = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[identity],
            provisioning_profiles=profiles,
            entitlements={
                "keychain-access-groups": ["AAAAAAAAAA.*"],
                "aps-environment": "production",
                "com.apple.security.application-groups": ["foo", "bar"],
                "com.apple.developer.devicecheck.appattest-environment": "production",
            },
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=None,
        )
        self.assertEqual(selected, SelectedProvisioningProfileInfo(expected, identity))

        selected, _ = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[identity],
            provisioning_profiles=profiles,
            entitlements={
                "aps-environment": "production",
                "com.apple.security.application-groups": ["foo", "bar"],
                "com.apple.developer.devicecheck.appattest-environment": "development",
            },
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=None,
        )
        self.assertEqual(selected, SelectedProvisioningProfileInfo(expected, identity))

        selected, diagnostic_info = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[identity],
            provisioning_profiles=[unexpected],
            entitlements={
                "aps-environment": "production",
                "com.apple.security.application-groups": ["foo", "xxx"],
            },
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=None,
        )
        self.assertIsNone(selected)
        self.verify_diagnostic_info_candidate_profile(
            diagnostic_info,
            "Expected entitlement item key `aps-environment` with value `production` not found in provisioning profile.",
        )

    def test_only_profiles_containing_valid_fingerprints_are_matched(self):
        info_plist = InfoPlistMetadata("com.company.application", None, False)
        valid_identity = CodeSigningIdentity(
            "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
            "iPhone Developer: Foo Bar (54321EDCBA)",
        )
        other_identity = CodeSigningIdentity(
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "iPhone Developer: Foo Bar (ABCDE12345)",
        )
        expected = ProvisioningProfileMetadata(
            Path("/foo"),
            "11111111-1111-1111-1111-111111111111",
            datetime.max,
            {"iOS"},
            {valid_identity.fingerprint, other_identity.fingerprint},
            {
                "application-identifier": "AAAAAAAAAA.*",
            },
        )
        unexpected = ProvisioningProfileMetadata(
            Path("/foo"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max,
            {"iOS"},
            {other_identity.fingerprint},
            {
                "application-identifier": "AAAAAAAAAA.com.company.application",
            },
        )

        profiles = [expected, unexpected]
        selected, _ = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[valid_identity],
            provisioning_profiles=profiles,
            entitlements={},
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=None,
        )
        self.assertEqual(
            selected, SelectedProvisioningProfileInfo(expected, valid_identity)
        )
        selected, diagnostic_info = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[valid_identity],
            provisioning_profiles=[unexpected],
            entitlements={},
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=None,
        )
        self.assertIsNone(selected)
        self.verify_diagnostic_info_candidate_profile(
            diagnostic_info,
            "Expected identity fingerprint not found in profile's certificate fingerprints `AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA`.",
        )

    def test_matches_specific_app(self):
        info_plist = InfoPlistMetadata("com.facebook.test", None, False)
        identity = CodeSigningIdentity(
            "fingerprint",
            "name",
        )
        expected = ProvisioningProfileMetadata(
            Path("/foo"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max,
            {"iOS"},
            {identity.fingerprint},
            {
                "application-identifier": "BBBBBBBBBB.com.facebook.test",
            },
        )
        profiles = [
            expected,
            ProvisioningProfileMetadata(
                Path("/foo"),
                "11111111-1111-1111-1111-111111111111",
                datetime.max,
                {"iOS"},
                {identity.fingerprint},
                {
                    "application-identifier": "BBBBBBBBBB.com.facebook.*",
                },
            ),
        ]
        selected, _ = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[identity],
            provisioning_profiles=profiles,
            entitlements={},
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=None,
        )
        self.assertEqual(selected, SelectedProvisioningProfileInfo(expected, identity))

        selected, _ = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[identity],
            provisioning_profiles=reversed(profiles),
            entitlements={},
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=None,
        )
        self.assertEqual(selected, SelectedProvisioningProfileInfo(expected, identity))

    def test_matches_wildcard(self):
        info_plist = InfoPlistMetadata("com.facebook.test", None, False)
        identity = CodeSigningIdentity(
            "fingerprint",
            "name",
        )
        expected = ProvisioningProfileMetadata(
            Path("/foo"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max,
            {"iOS"},
            {identity.fingerprint},
            {
                "application-identifier": "BBBBBBBBBB.*",
            },
        )
        selected, _ = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[identity],
            provisioning_profiles=[expected],
            entitlements=None,
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=None,
        )
        self.assertEqual(selected, SelectedProvisioningProfileInfo(expected, identity))

    def test_force_included_app_entitlements(self):
        info_plist = InfoPlistMetadata("com.facebook.test", None, False)
        identity = CodeSigningIdentity(
            "fingerprint",
            "name",
        )
        profile = ProvisioningProfileMetadata(
            Path("/foo"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max,
            {"iOS"},
            {identity.fingerprint},
            {
                "application-identifier": "AAAAAAAAAA.com.facebook.test",
                "keychain-access-groups": ["AAAAAAAAAA.*"],
                "aps-environment": "production",
            },
        )
        selected, _ = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[identity],
            provisioning_profiles=[profile],
            entitlements={
                # Force included key, even if not present in the profile
                "application-identifier": "AAAAAAAAAA.com.facebook.BuckApp",
                "keychain-access-groups": ["AAAAAAAAAA.*"],
                "aps-environment": "production",
            },
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=None,
        )
        self.assertIsNotNone(selected)

    def test_unmatched_app_entitlement(self):
        info_plist = InfoPlistMetadata("com.facebook.test", None, False)
        identity = CodeSigningIdentity(
            "fingerprint",
            "name",
        )
        profile = ProvisioningProfileMetadata(
            Path("/foo"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max,
            {"iOS"},
            {identity.fingerprint},
            {
                "application-identifier": "AAAAAAAAAA.com.facebook.test",
                "keychain-access-groups": ["AAAAAAAAAA.*"],
                "aps-environment": "production",
            },
        )
        selected, diagnostic_info = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[identity],
            provisioning_profiles=[profile],
            entitlements={
                "keychain-access-groups": ["AAAAAAAAAA.*"],
                "aps-environment": "production",
                "com.made.up.entitlement": "buck",
            },
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=None,
        )
        self.assertIsNone(selected)
        self.verify_diagnostic_info_candidate_profile(
            diagnostic_info,
            "Expected entitlement item key `com.made.up.entitlement` with value `buck` not found in provisioning profile.",
        )

    def test_wildcard_app_entitlement_matches_any_value(self):
        info_plist = InfoPlistMetadata("com.facebook.test", None, False)
        identity = CodeSigningIdentity(
            "fingerprint",
            "name",
        )
        profile = ProvisioningProfileMetadata(
            Path("/foo"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max,
            {"iOS"},
            {identity.fingerprint},
            {
                "application-identifier": "AAAAAAAAAA.com.facebook.test",
                "keychain-access-groups": ["AAAAAAAAAA.*"],
                "aps-environment": "production",
                "com.apple.security.hardened-process.enhanced-security-version": "*",
                "com.apple.security.hardened-process.platform-restrictions": "*",
            },
        )
        selected, _ = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[identity],
            provisioning_profiles=[profile],
            entitlements={
                "com.apple.security.hardened-process.enhanced-security-version": "1",
                "com.apple.security.hardened-process.platform-restrictions": "2",
            },
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=None,
        )
        self.assertEqual(selected, SelectedProvisioningProfileInfo(profile, identity))

    def test_unmatched_wildcard_app_entitlement(self):
        info_plist = InfoPlistMetadata("com.facebook.test", None, False)
        identity = CodeSigningIdentity(
            "fingerprint",
            "name",
        )
        profile = ProvisioningProfileMetadata(
            Path("/foo"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max,
            {"iOS"},
            {identity.fingerprint},
            {
                "application-identifier": "AAAAAAAAAA.com.facebook.test",
                "keychain-access-groups": ["AAAAAAAAAA.*"],
                "aps-environment": "production",
            },
        )
        selected, diagnostic_info = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[identity],
            provisioning_profiles=[profile],
            entitlements={
                "com.apple.security.hardened-process.enhanced-security-version": "1",
                "com.apple.security.hardened-process.platform-restrictions": "2",
            },
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=None,
        )
        self.assertIsNone(selected)
        self.verify_diagnostic_info_candidate_profile(
            diagnostic_info,
            "Expected entitlement item key `com.apple.security.hardened-process.enhanced-security-version` with value `1` not found in provisioning profile.",
        )

    def test_no_check_certificates_skips_identity_matching(self):
        info_plist = InfoPlistMetadata("com.company.application", None, False)
        profile = ProvisioningProfileMetadata(
            Path("/foo"),
            "00000000-0000-0000-0000-000000000000",
            datetime.max,
            {"iOS"},
            {"some_fingerprint"},
            {"application-identifier": "AAAAAAAAAA.com.company.application"},
        )
        # Empty identities list - would fail without no_check_certificates
        selected, _ = select_best_provisioning_profile_core(
            info_plist_metadata=info_plist,
            code_signing_identities=[],
            provisioning_profiles=[profile],
            entitlements={},
            platform=ApplePlatform.ios_device,
            strict_search=False,
            provisioning_profile_filter=None,
            no_check_certificates=True,
        )
        self.assertIsNotNone(selected)
        self.assertEqual(selected.profile, profile)
        self.assertEqual(selected.identity.fingerprint, "some_fingerprint")
        self.assertEqual(selected.identity.subject_common_name, "Unknown")


class TestEntitlementMatching(unittest.TestCase):
    def test_app_attest_environment_matches_allowed_values(self):
        key = "com.apple.developer.devicecheck.appattest-environment"
        for requested, permitted, expected in [
            ("development", ["development", "production"], True),
            ("production", ["development", "production"], True),
            ("development", ["development"], True),
            ("production", ["production"], True),
            ("development", ["production"], False),
            ("production", ["development"], False),
            ("unsupported", ["development", "production"], False),
            ("development", [], False),
            ("development", None, False),
            ("development", "development", True),
            ("production", "production", True),
            ("development", "production", False),
        ]:
            with self.subTest(requested=requested, permitted=permitted):
                self.assertEqual(
                    _matches_or_array_is_subset_of(
                        key, requested, permitted, ApplePlatform.ios_device
                    ),
                    expected,
                )

    def test_other_string_entitlements_match_allowed_values(self):
        for key in ["aps-environment", "another-entitlement"]:
            for requested, permitted, expected in [
                ("development", ["development", "production"], True),
                ("production", ["development", "production"], True),
                ("development", ["production"], False),
                ("unsupported", ["development", "production"], False),
                ("development", [], False),
            ]:
                with self.subTest(key=key, requested=requested, permitted=permitted):
                    self.assertEqual(
                        _matches_or_array_is_subset_of(
                            key, requested, permitted, ApplePlatform.ios_device
                        ),
                        expected,
                    )

    def test_string_entitlements_reject_non_string_allowlists(self):
        for key in [
            "com.apple.developer.devicecheck.appattest-environment",
            "aps-environment",
        ]:
            for permitted in [
                ["development", 1],
                ["development", True],
                ["development", None],
                ["development", ["production"]],
                ["development", {"environment": "production"}],
                [1, 2],
            ]:
                with self.subTest(key=key, permitted=permitted):
                    self.assertFalse(
                        _matches_or_array_is_subset_of(
                            key, "development", permitted, ApplePlatform.ios_device
                        )
                    )

    def test_non_string_scalar_entitlements_do_not_match_profile_arrays(self):
        for requested, permitted in [(1, [1]), (True, [True]), (None, [None])]:
            with self.subTest(requested=requested, permitted=permitted):
                self.assertFalse(
                    _matches_or_array_is_subset_of(
                        "another-entitlement",
                        requested,
                        permitted,
                        ApplePlatform.ios_device,
                    )
                )

    def test_existing_matching_behavior_is_preserved(self):
        for requested, permitted, expected in [
            (["foo"], ["foo", "bar"], True),
            (["foo", "baz"], ["foo", "bar"], False),
            ("development", "development", True),
            ("development", "production", False),
            ("development", "*", True),
            (True, True, True),
            (True, False, False),
            (None, None, True),
        ]:
            with self.subTest(requested=requested, permitted=permitted):
                self.assertEqual(
                    _matches_or_array_is_subset_of(
                        "another-entitlement",
                        requested,
                        permitted,
                        ApplePlatform.ios_device,
                    ),
                    expected,
                )
