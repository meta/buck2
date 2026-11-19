# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import os
import plistlib
import tempfile
import unittest
from datetime import datetime
from pathlib import Path

from .prepare_code_signing_entitlements import prepare_code_signing_entitlements
from .provisioning_profile_metadata import ProvisioningProfileMetadata


class Test(unittest.TestCase):
    def test_minimal_entitlements_generated_based_on_provisioning_profile(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            profile = ProvisioningProfileMetadata(
                Path("/foo"),
                "00000000-0000-0000-0000-000000000000",
                datetime.max,
                {"iOS"},
                {},
                {
                    "application-identifier": "ABCDEFGHIJ.*",
                    "com.apple.developer.aps-environment": "development",
                },
            )
            result = prepare_code_signing_entitlements(
                None, "com.company.application", profile, tmp_dir
            )
            with open(result, mode="rb") as result_file:
                self.assertEqual(
                    plistlib.load(result_file),
                    {
                        "application-identifier": "ABCDEFGHIJ.com.company.application",
                        "com.apple.developer.aps-environment": "development",
                        "keychain-access-groups": [
                            "ABCDEFGHIJ.com.company.application"
                        ],
                    },
                )

    def test_suffix_applied_to_auto_generated_entitlements(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            profile = ProvisioningProfileMetadata(
                Path("/foo"),
                "00000000-0000-0000-0000-000000000000",
                datetime.max,
                {"iOS"},
                {},
                {
                    "application-identifier": "ABCDEFGHIJ.*",
                    "com.apple.developer.aps-environment": "development",
                },
            )
            result = prepare_code_signing_entitlements(
                None,
                "com.company.application",
                profile,
                tmp_dir,
                entitlements_suffixed_key_map={"keychain-access-groups": ".suffix"},
            )
            with open(result, mode="rb") as result_file:
                self.assertEqual(
                    plistlib.load(result_file),
                    {
                        "application-identifier": "ABCDEFGHIJ.com.company.application",
                        "com.apple.developer.aps-environment": "development",
                        "keychain-access-groups": [
                            "ABCDEFGHIJ.com.company.application.suffix"
                        ],
                    },
                )

    def test_suffix_applied_to_user_provided_entitlements(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            entitlements = {
                "foo": "bar",
                "keychain-access-groups": ["group1", "group2"],
            }
            entitlements_path = os.path.join(tmp_dir, "Entitlements.plist")
            with open(entitlements_path, mode="wb") as entitlements_file:
                plistlib.dump(entitlements, entitlements_file, fmt=plistlib.FMT_XML)
            profile = ProvisioningProfileMetadata(
                Path("/foo"),
                "00000000-0000-0000-0000-000000000000",
                datetime.max,
                {"iOS"},
                {},
                {
                    "application-identifier": "ABCDEFGHIJ.com.company.application",
                    "com.apple.developer.aps-environment": "development",
                    "should.be.ignored": "dummy",
                },
            )
            result = prepare_code_signing_entitlements(
                entitlements_path,
                "com.company.application",
                profile,
                tmp_dir,
                entitlements_suffixed_key_map={"keychain-access-groups": ".mysuffix"},
            )
            with open(result, "rb") as result_file:
                self.assertEqual(
                    plistlib.load(result_file),
                    {
                        "foo": "bar",
                        "application-identifier": "ABCDEFGHIJ.com.company.application",
                        "com.apple.developer.aps-environment": "development",
                        "keychain-access-groups": [
                            "group1.mysuffix",
                            "group2.mysuffix",
                        ],
                    },
                )

    def test_no_modification_when_suffix_is_none(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            profile = ProvisioningProfileMetadata(
                Path("/foo"),
                "00000000-0000-0000-0000-000000000000",
                datetime.max,
                {"iOS"},
                {},
                {
                    "application-identifier": "ABCDEFGHIJ.*",
                    "com.apple.developer.aps-environment": "development",
                },
            )
            result = prepare_code_signing_entitlements(
                None,
                "com.company.application",
                profile,
                tmp_dir,
                entitlements_suffixed_key_map={},
            )
            with open(result, mode="rb") as result_file:
                self.assertEqual(
                    plistlib.load(result_file),
                    {
                        "application-identifier": "ABCDEFGHIJ.com.company.application",
                        "com.apple.developer.aps-environment": "development",
                        "keychain-access-groups": [
                            "ABCDEFGHIJ.com.company.application"
                        ],
                    },
                )

    def test_no_modification_when_keychain_access_groups_absent(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            entitlements = {"foo": "bar"}
            entitlements_path = os.path.join(tmp_dir, "Entitlements.plist")
            with open(entitlements_path, mode="wb") as entitlements_file:
                plistlib.dump(entitlements, entitlements_file, fmt=plistlib.FMT_XML)
            profile = ProvisioningProfileMetadata(
                Path("/foo"),
                "00000000-0000-0000-0000-000000000000",
                datetime.max,
                {"iOS"},
                {},
                {
                    "application-identifier": "ABCDEFGHIJ.com.company.application",
                },
            )
            result = prepare_code_signing_entitlements(
                entitlements_path,
                "com.company.application",
                profile,
                tmp_dir,
                entitlements_suffixed_key_map={"keychain-access-groups": ".suffix"},
            )
            with open(result, "rb") as result_file:
                self.assertEqual(
                    plistlib.load(result_file),
                    {
                        "foo": "bar",
                        "application-identifier": "ABCDEFGHIJ.com.company.application",
                    },
                )

    def test_suffix_applied_to_string_entitlement_value(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            entitlements = {
                "com.apple.developer.ubiquity-kvstore-identifier": "ABCDEFGHIJ.com.company.application",
            }
            entitlements_path = os.path.join(tmp_dir, "Entitlements.plist")
            with open(entitlements_path, mode="wb") as entitlements_file:
                plistlib.dump(entitlements, entitlements_file, fmt=plistlib.FMT_XML)
            profile = ProvisioningProfileMetadata(
                Path("/foo"),
                "00000000-0000-0000-0000-000000000000",
                datetime.max,
                {"iOS"},
                {},
                {
                    "application-identifier": "ABCDEFGHIJ.com.company.application",
                },
            )
            result = prepare_code_signing_entitlements(
                entitlements_path,
                "com.company.application",
                profile,
                tmp_dir,
                entitlements_suffixed_key_map={
                    "com.apple.developer.ubiquity-kvstore-identifier": ".mysuffix"
                },
            )
            with open(result, "rb") as result_file:
                self.assertEqual(
                    plistlib.load(result_file),
                    {
                        "application-identifier": "ABCDEFGHIJ.com.company.application",
                        "com.apple.developer.ubiquity-kvstore-identifier": "ABCDEFGHIJ.com.company.application.mysuffix",
                    },
                )

    def test_removed_keys_removes_existing_keys(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            entitlements = {
                "foo": "bar",
                "keychain-access-groups": ["group1", "group2"],
                "com.apple.developer.icloud-services": ["CloudDocuments"],
            }
            entitlements_path = os.path.join(tmp_dir, "Entitlements.plist")
            with open(entitlements_path, mode="wb") as entitlements_file:
                plistlib.dump(entitlements, entitlements_file, fmt=plistlib.FMT_XML)
            profile = ProvisioningProfileMetadata(
                Path("/foo"),
                "00000000-0000-0000-0000-000000000000",
                datetime.max,
                {"iOS"},
                {},
                {
                    "application-identifier": "ABCDEFGHIJ.com.company.application",
                },
            )
            result = prepare_code_signing_entitlements(
                entitlements_path,
                "com.company.application",
                profile,
                tmp_dir,
                entitlements_removed_keys=["com.apple.developer.icloud-services"],
            )
            with open(result, "rb") as result_file:
                self.assertEqual(
                    plistlib.load(result_file),
                    {
                        "foo": "bar",
                        "application-identifier": "ABCDEFGHIJ.com.company.application",
                        "keychain-access-groups": ["group1", "group2"],
                    },
                )

    def test_removed_keys_no_modification_when_key_absent(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            entitlements = {
                "foo": "bar",
            }
            entitlements_path = os.path.join(tmp_dir, "Entitlements.plist")
            with open(entitlements_path, mode="wb") as entitlements_file:
                plistlib.dump(entitlements, entitlements_file, fmt=plistlib.FMT_XML)
            profile = ProvisioningProfileMetadata(
                Path("/foo"),
                "00000000-0000-0000-0000-000000000000",
                datetime.max,
                {"iOS"},
                {},
                {
                    "application-identifier": "ABCDEFGHIJ.com.company.application",
                },
            )
            result = prepare_code_signing_entitlements(
                entitlements_path,
                "com.company.application",
                profile,
                tmp_dir,
                entitlements_removed_keys=["nonexistent-key"],
            )
            with open(result, "rb") as result_file:
                self.assertEqual(
                    plistlib.load(result_file),
                    {
                        "foo": "bar",
                        "application-identifier": "ABCDEFGHIJ.com.company.application",
                    },
                )

    def test_entitlements_enriched_by_profile(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            entitlements = {"foo": "bar"}
            entitlements_path = os.path.join(tmp_dir, "Entitlements.plist")
            with open(entitlements_path, mode="wb") as entitlements_file:
                plistlib.dump(entitlements, entitlements_file, fmt=plistlib.FMT_XML)
            profile = ProvisioningProfileMetadata(
                Path("/foo"),
                "00000000-0000-0000-0000-000000000000",
                datetime.max,
                {"iOS"},
                {},
                {
                    "application-identifier": "ABCDEFGHIJ.com.company.application",
                    "com.apple.developer.aps-environment": "development",
                    "should.be.ignored": "dummy",
                },
            )
            result = prepare_code_signing_entitlements(
                entitlements_path, "com.company.application", profile, tmp_dir
            )
            with open(result, "rb") as result_file:
                self.assertEqual(
                    plistlib.load(result_file),
                    {
                        "foo": "bar",
                        "application-identifier": "ABCDEFGHIJ.com.company.application",
                        "com.apple.developer.aps-environment": "development",
                    },
                )

    def _assert_prepared_entitlements(
        self, input_entitlements, expected, profile_entitlements=None, **kwargs
    ):
        if profile_entitlements is None:
            profile_entitlements = {
                "application-identifier": "ABCDEFGHIJ.com.company.application",
            }
        with tempfile.TemporaryDirectory() as tmp_dir:
            if input_entitlements is not None:
                entitlements_path = os.path.join(tmp_dir, "Entitlements.plist")
                with open(entitlements_path, mode="wb") as f:
                    plistlib.dump(input_entitlements, f, fmt=plistlib.FMT_XML)
            else:
                entitlements_path = None
            profile = ProvisioningProfileMetadata(
                Path("/foo"),
                "00000000-0000-0000-0000-000000000000",
                datetime.max,
                {"iOS"},
                {},
                profile_entitlements,
            )
            result = prepare_code_signing_entitlements(
                entitlements_path,
                "com.company.application",
                profile,
                tmp_dir,
                **kwargs,
            )
            with open(result, "rb") as f:
                self.assertEqual(plistlib.load(f), expected)

    def test_removed_values_map_removes_from_list(self):
        self._assert_prepared_entitlements(
            input_entitlements={
                "foo": "bar",
                "keychain-access-groups": ["group1", "group2", "group3"],
            },
            expected={
                "foo": "bar",
                "application-identifier": "ABCDEFGHIJ.com.company.application",
                "keychain-access-groups": ["group2"],
            },
            entitlements_removed_values_map={
                "keychain-access-groups": ["group1", "group3"]
            },
        )

    def test_removed_values_map_removes_from_dict(self):
        self._assert_prepared_entitlements(
            input_entitlements={
                "foo": "bar",
                "com.apple.developer.icloud-container-environment": {
                    "Production": True,
                    "Development": True,
                    "Staging": True,
                },
            },
            expected={
                "foo": "bar",
                "application-identifier": "ABCDEFGHIJ.com.company.application",
                "com.apple.developer.icloud-container-environment": {
                    "Production": True,
                },
            },
            entitlements_removed_values_map={
                "com.apple.developer.icloud-container-environment": [
                    "Development",
                    "Staging",
                ]
            },
        )

    def test_removed_values_map_no_modification_when_key_absent(self):
        self._assert_prepared_entitlements(
            input_entitlements={"foo": "bar"},
            expected={
                "foo": "bar",
                "application-identifier": "ABCDEFGHIJ.com.company.application",
            },
            entitlements_removed_values_map={"nonexistent-key": ["value1"]},
        )

    def test_removed_values_map_no_modification_when_value_absent(self):
        self._assert_prepared_entitlements(
            input_entitlements={
                "foo": "bar",
                "keychain-access-groups": ["group1", "group2"],
            },
            expected={
                "foo": "bar",
                "application-identifier": "ABCDEFGHIJ.com.company.application",
                "keychain-access-groups": ["group1", "group2"],
            },
            entitlements_removed_values_map={
                "keychain-access-groups": ["nonexistent-value"]
            },
        )

    def test_removed_values_map_empty_map(self):
        self._assert_prepared_entitlements(
            input_entitlements=None,
            expected={
                "application-identifier": "ABCDEFGHIJ.com.company.application",
                "com.apple.developer.aps-environment": "development",
                "keychain-access-groups": ["ABCDEFGHIJ.com.company.application"],
            },
            profile_entitlements={
                "application-identifier": "ABCDEFGHIJ.*",
                "com.apple.developer.aps-environment": "development",
            },
            entitlements_removed_values_map={},
        )
