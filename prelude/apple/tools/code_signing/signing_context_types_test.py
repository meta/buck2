# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import datetime
import json
import unittest
from pathlib import Path

from .identity import CodeSigningIdentity
from .info_plist_metadata import InfoPlistMetadata
from .provisioning_profile_metadata import ProvisioningProfileMetadata
from .provisioning_profile_selection import SelectedProvisioningProfileInfo
from .signing_context_types import (
    AdhocSigningContext,
    SigningContextWithProfileSelection,
)


class TestIdentitySerialization(unittest.TestCase):
    def test_round_trip(self):
        identity = CodeSigningIdentity(
            fingerprint="ABCDEF1234567890ABCDEF1234567890ABCDEF12",
            subject_common_name="Apple Development: Test",
        )
        data = identity.to_dict()
        restored = CodeSigningIdentity.from_dict(data)
        self.assertEqual(identity, restored)
        json_str = json.dumps(data)
        loaded = json.loads(json_str)
        restored2 = CodeSigningIdentity.from_dict(loaded)
        self.assertEqual(identity, restored2)

    def test_rejects_unknown_fields(self):
        with self.assertRaisesRegex(ValueError, "unexpected=.*team_id"):
            CodeSigningIdentity.from_dict(
                {
                    "fingerprint": "ABC",
                    "subject_common_name": "Developer",
                    "team_id": "TEAM",
                }
            )

    def test_rejects_invalid_field_type(self):
        with self.assertRaisesRegex(ValueError, "fingerprint must be a string"):
            CodeSigningIdentity.from_dict(
                {"fingerprint": 123, "subject_common_name": "Developer"}
            )


class TestInfoPlistMetadataSerialization(unittest.TestCase):
    def test_round_trip(self):
        metadata = InfoPlistMetadata(
            bundle_id="com.test.app", bundle_type="APPL", is_watchos_app=False
        )
        data = metadata.to_dict()
        restored = InfoPlistMetadata.from_dict(data)
        self.assertEqual(metadata, restored)

    def test_optional_fields(self):
        data = {"bundle_id": "com.test", "bundle_type": None, "is_watchos_app": False}
        restored = InfoPlistMetadata.from_dict(data)
        self.assertEqual(restored.bundle_id, "com.test")
        self.assertIsNone(restored.bundle_type)

    def test_requires_watchos_field(self):
        with self.assertRaisesRegex(ValueError, "missing=.*is_watchos_app"):
            InfoPlistMetadata.from_dict({"bundle_id": "com.test", "bundle_type": None})


class TestProvisioningProfileMetadataSerialization(unittest.TestCase):
    def test_round_trip_basic(self):
        expiration = datetime.datetime(2025, 1, 1, 12, 0, 0)
        creation = datetime.datetime(2024, 1, 1, 12, 0, 0)
        profile = ProvisioningProfileMetadata(
            file_path=Path("/tmp/profile.mobileprovision"),
            uuid="UUID-1234",
            expiration_date=expiration,
            platforms=frozenset(["iOS"]),
            developer_certificate_fingerprints=frozenset(["ABCDEF1234567890"]),
            entitlements={"application-identifier": "TEAM.com.test.app"},
            creation_date=creation,
            provisions_all_devices=False,
            provisioned_devices=["device1"],
        )
        data = profile.to_dict()
        restored = ProvisioningProfileMetadata.from_dict(data)
        self.assertEqual(restored.uuid, profile.uuid)
        self.assertEqual(restored.expiration_date, profile.expiration_date)
        self.assertEqual(restored.creation_date, profile.creation_date)
        self.assertEqual(restored.platforms, profile.platforms)
        self.assertEqual(
            restored.developer_certificate_fingerprints,
            profile.developer_certificate_fingerprints,
        )
        self.assertEqual(restored.entitlements, profile.entitlements)

    def test_bytes_and_datetime_entitlements(self):
        expiration = datetime.datetime(2025, 1, 1, 12, 0, 0)
        profile = ProvisioningProfileMetadata(
            file_path=Path("/tmp/profile.mobileprovision"),
            uuid="UUID-BYTES",
            expiration_date=expiration,
            platforms=frozenset(["iOS"]),
            developer_certificate_fingerprints=frozenset(["ABC"]),
            entitlements={
                "data-key": b"\x00\x01\x02binary",
                "date-key": datetime.datetime(2024, 6, 1, 10, 0, 0),
                "nested": {"inner-bytes": b"inner"},
            },
            creation_date=None,
            provisions_all_devices=False,
            provisioned_devices=None,
        )
        data = profile.to_dict()
        json_str = json.dumps(data)
        loaded = json.loads(json_str)
        restored = ProvisioningProfileMetadata.from_dict(loaded)
        self.assertEqual(restored.entitlements["data-key"], b"\x00\x01\x02binary")
        self.assertEqual(
            restored.entitlements["date-key"],
            datetime.datetime(2024, 6, 1, 10, 0, 0),
        )
        self.assertEqual(restored.entitlements["nested"]["inner-bytes"], b"inner")

    def test_rejects_missing_fields(self):
        profile = self._profile()
        data = profile.to_dict()
        del data["platforms"]
        with self.assertRaisesRegex(ValueError, "missing=.*platforms"):
            ProvisioningProfileMetadata.from_dict(data)

    def test_rejects_malformed_entitlement_marker(self):
        profile = self._profile()
        data = profile.to_dict()
        data["entitlements"] = {"data-key": {"__apple_code_signing_bytes_b64__": 123}}
        with self.assertRaisesRegex(ValueError, "must be a string"):
            ProvisioningProfileMetadata.from_dict(data)

    def test_marker_shaped_entitlement_dicts_round_trip(self):
        profile = self._profile()
        profile.entitlements = {
            "bytes-marker": {"__apple_code_signing_bytes_b64__": "YWJj"},
            "datetime-marker": {
                "__apple_code_signing_datetime_iso__": "2025-01-01T12:00:00"
            },
            "escape-marker": {
                "__apple_code_signing_escaped_dict__": {"nested": "value"}
            },
        }

        serialized = json.loads(json.dumps(profile.to_dict()))
        restored = ProvisioningProfileMetadata.from_dict(serialized)

        self.assertEqual(profile.entitlements, restored.entitlements)

    def _profile(self) -> ProvisioningProfileMetadata:
        return ProvisioningProfileMetadata(
            file_path=Path("/tmp/profile.mobileprovision"),
            uuid="UUID-STRICT",
            expiration_date=datetime.datetime(2025, 1, 1, 12, 0, 0),
            platforms=frozenset(["iOS"]),
            developer_certificate_fingerprints=frozenset(["ABC"]),
            entitlements={},
            creation_date=None,
            provisions_all_devices=False,
            provisioned_devices=None,
        )


class TestSelectedProvisioningProfileInfoSerialization(unittest.TestCase):
    def test_round_trip(self):
        expiration = datetime.datetime(2025, 1, 1, 12, 0, 0)
        profile = ProvisioningProfileMetadata(
            file_path=Path("/tmp/profile.mobileprovision"),
            uuid="UUID-1234",
            expiration_date=expiration,
            platforms=frozenset(["iOS"]),
            developer_certificate_fingerprints=frozenset(["ABC"]),
            entitlements={},
            creation_date=None,
            provisions_all_devices=False,
            provisioned_devices=None,
        )
        identity = CodeSigningIdentity(
            fingerprint="ABC", subject_common_name="Apple Development: Test"
        )
        data_no_file = {
            "identity": identity.to_dict(),
            "profile": profile.to_dict(),
        }
        json_str = json.dumps(data_no_file)
        loaded = json.loads(json_str)
        restored = SelectedProvisioningProfileInfo.from_dict(loaded)
        self.assertEqual(restored.profile.uuid, "UUID-1234")
        self.assertEqual(restored.identity.fingerprint, "ABC")


class TestSigningContextWithProfileSelectionSerialization(unittest.TestCase):
    def test_round_trip(self):
        expiration = datetime.datetime(2025, 1, 1, 12, 0, 0)
        profile = ProvisioningProfileMetadata(
            file_path=Path("/tmp/profile.mobileprovision"),
            uuid="UUID-1234",
            expiration_date=expiration,
            platforms=frozenset(["iOS"]),
            developer_certificate_fingerprints=frozenset(["ABC"]),
            entitlements={},
            creation_date=None,
            provisions_all_devices=False,
            provisioned_devices=None,
        )
        identity = CodeSigningIdentity(fingerprint="ABC", subject_common_name="Dev")
        selected = SelectedProvisioningProfileInfo(profile=profile, identity=identity)
        metadata = InfoPlistMetadata(
            bundle_id="com.test.app", bundle_type="APPL", is_watchos_app=False
        )
        ctx = SigningContextWithProfileSelection(
            info_plist_source=Path("/tmp/Info.plist"),
            info_plist_destination=Path("Info.plist"),
            info_plist_metadata=metadata,
            selected_profile_info=selected,
        )
        dict_without_file = {
            "info_plist_source": str(ctx.info_plist_source),
            "info_plist_destination": str(ctx.info_plist_destination),
            "info_plist_metadata": metadata.to_dict(),
            "selected_profile_info": {
                "identity": identity.to_dict(),
                "profile": profile.to_dict(),
            },
        }
        json_str = json.dumps(dict_without_file)
        loaded = json.loads(json_str)
        restored = SigningContextWithProfileSelection.from_dict(loaded)
        self.assertEqual(restored.info_plist_source, Path("/tmp/Info.plist"))
        self.assertEqual(restored.info_plist_metadata.bundle_id, "com.test.app")
        self.assertEqual(restored.selected_profile_info.profile.uuid, "UUID-1234")


class TestAdhocSigningContextSerialization(unittest.TestCase):
    def test_adhoc_without_profile(self):
        ctx = AdhocSigningContext(codesign_identity="-", profile_selection_context=None)
        data = ctx.to_dict()
        self.assertIsNone(data["profile_selection_context"])
        json_str = json.dumps(data)
        loaded = json.loads(json_str)
        restored = AdhocSigningContext.from_dict(loaded)
        self.assertEqual(restored.codesign_identity, "-")
        self.assertIsNone(restored.profile_selection_context)

    def test_rejects_null_codesign_identity(self):
        with self.assertRaisesRegex(ValueError, "codesign_identity must be a string"):
            AdhocSigningContext.from_dict(
                {
                    "codesign_identity": None,
                    "profile_selection_context": None,
                }
            )

    def test_adhoc_with_profile_explicit_none_check(self):
        # Ensures from_dict uses `is not None` check, not truthiness, so empty dict case would be preserved if ever occurs
        expiration = datetime.datetime(2025, 1, 1, 12, 0, 0)
        profile = ProvisioningProfileMetadata(
            file_path=Path("/tmp/profile.mobileprovision"),
            uuid="UUID-5678",
            expiration_date=expiration,
            platforms=frozenset(["iOS"]),
            developer_certificate_fingerprints=frozenset(["AAA"]),
            entitlements={},
            creation_date=None,
            provisions_all_devices=True,
            provisioned_devices=None,
        )
        identity = CodeSigningIdentity(fingerprint="AAA", subject_common_name="Adhoc")
        selected = SelectedProvisioningProfileInfo(profile=profile, identity=identity)
        metadata = InfoPlistMetadata(
            bundle_id="com.test.app", bundle_type="APPL", is_watchos_app=False
        )
        distribution = SigningContextWithProfileSelection(
            info_plist_source=Path("/tmp/Info.plist"),
            info_plist_destination=Path("Info.plist"),
            info_plist_metadata=metadata,
            selected_profile_info=selected,
        )
        adhoc = AdhocSigningContext(
            codesign_identity="Apple Development: Adhoc",
            profile_selection_context=distribution,
        )
        data = adhoc.to_dict()
        json_str = json.dumps(
            {
                "codesign_identity": data["codesign_identity"],
                "profile_selection_context": {
                    "info_plist_source": str(distribution.info_plist_source),
                    "info_plist_destination": str(distribution.info_plist_destination),
                    "info_plist_metadata": metadata.to_dict(),
                    "selected_profile_info": {
                        "identity": identity.to_dict(),
                        "profile": profile.to_dict(),
                    },
                },
            }
        )
        loaded = json.loads(json_str)
        restored = AdhocSigningContext.from_dict(loaded)
        self.assertEqual(restored.codesign_identity, "Apple Development: Adhoc")
        assert restored.profile_selection_context is not None
        self.assertEqual(
            restored.profile_selection_context.selected_profile_info.profile.uuid,
            "UUID-5678",
        )
