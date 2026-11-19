# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

from __future__ import annotations

import base64
import binascii
import hashlib
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, cast, Optional

from apple.tools.plistlib_utils import detect_format_and_loads

from .app_id import AppId
from .serialization import (
    expect_dict,
    expect_keys,
    expect_optional_bool,
    expect_optional_str_list,
    expect_str,
    expect_str_list,
)

_ENTITLEMENTS_BYTES_MARKER = "__apple_code_signing_bytes_b64__"
_ENTITLEMENTS_DATETIME_MARKER = "__apple_code_signing_datetime_iso__"
_ENTITLEMENTS_ESCAPED_DICT_MARKER = "__apple_code_signing_escaped_dict__"
_ENTITLEMENTS_RESERVED_MARKERS = (
    _ENTITLEMENTS_BYTES_MARKER,
    _ENTITLEMENTS_DATETIME_MARKER,
    _ENTITLEMENTS_ESCAPED_DICT_MARKER,
)


def _serialize_datetime(dt: Optional[datetime]) -> Optional[str]:
    if dt is None:
        return None
    return dt.isoformat()


def _deserialize_datetime(value: object, path: str) -> Optional[datetime]:
    if value is None:
        return None
    return datetime.fromisoformat(expect_str(value, path))


def _make_entitlements_json_safe(obj: Any) -> Any:
    """Encode plist-only scalar types as tagged JSON objects."""
    if isinstance(obj, bytes):
        return {_ENTITLEMENTS_BYTES_MARKER: base64.standard_b64encode(obj).decode()}
    if isinstance(obj, datetime):
        return {_ENTITLEMENTS_DATETIME_MARKER: obj.isoformat()}
    if isinstance(obj, dict):
        encoded = {k: _make_entitlements_json_safe(v) for k, v in obj.items()}
        # A marker-shaped dictionary is valid plist data and must not be
        # mistaken for one of the tagged scalar representations when decoded.
        if len(obj) == 1 and next(iter(obj)) in _ENTITLEMENTS_RESERVED_MARKERS:
            return {_ENTITLEMENTS_ESCAPED_DICT_MARKER: encoded}
        return encoded
    if isinstance(obj, (list, tuple)):
        return [_make_entitlements_json_safe(x) for x in obj]
    return obj


def _restore_entitlements_from_json_safe(obj: object) -> Any:
    if isinstance(obj, dict):
        data = expect_dict(obj, "entitlements")
        if set(data) == {_ENTITLEMENTS_BYTES_MARKER}:
            raw = obj[_ENTITLEMENTS_BYTES_MARKER]
            encoded = expect_str(raw, f"entitlements.{_ENTITLEMENTS_BYTES_MARKER}")
            try:
                return base64.b64decode(encoded, validate=True)
            except binascii.Error as error:
                raise ValueError("Invalid base64 entitlement value") from error
        if set(data) == {_ENTITLEMENTS_DATETIME_MARKER}:
            raw = obj[_ENTITLEMENTS_DATETIME_MARKER]
            return datetime.fromisoformat(
                expect_str(raw, f"entitlements.{_ENTITLEMENTS_DATETIME_MARKER}")
            )
        if set(data) == {_ENTITLEMENTS_ESCAPED_DICT_MARKER}:
            escaped = expect_dict(
                data[_ENTITLEMENTS_ESCAPED_DICT_MARKER],
                f"entitlements.{_ENTITLEMENTS_ESCAPED_DICT_MARKER}",
            )
            return {
                key: _restore_entitlements_from_json_safe(value)
                for key, value in escaped.items()
            }
        return {
            key: _restore_entitlements_from_json_safe(value)
            for key, value in data.items()
        }
    if isinstance(obj, list):
        return [_restore_entitlements_from_json_safe(value) for value in obj]
    if obj is None or isinstance(obj, (bool, int, float, str)):
        return obj
    raise ValueError(f"Unsupported entitlement JSON value: {type(obj).__name__}")


@dataclass
class ProvisioningProfileMetadata:
    # Path to the provisioning profile file
    file_path: Path
    uuid: str
    # Naïve object with ignored timezone, see https://bugs.python.org/msg110249
    expiration_date: datetime
    platforms: frozenset[str]
    # Let's agree they are uppercased
    developer_certificate_fingerprints: frozenset[str]
    entitlements: dict[str, Any]
    # Naïve object with ignored timezone (same as expiration_date)
    creation_date: Optional[datetime] = None
    provisions_all_devices: Optional[bool] = None
    provisioned_devices: Optional[list[str]] = None

    _mergeable_entitlements_keys: frozenset[str] = frozenset(
        [
            "application-identifier",
            "beta-reports-active",
            "get-task-allow",
            "com.apple.developer.aps-environment",
            "com.apple.developer.team-identifier",
        ]
    )

    # See `ProvisioningProfileMetadataFactory::getAppIDFromEntitlements` from `ProvisioningProfileMetadataFactory.java` in Buck v1
    def get_app_id(self) -> AppId:
        maybe_app_id = self.entitlements.get(
            "application-identifier"
        ) or self.entitlements.get("com.apple.application-identifier")
        if not maybe_app_id:
            raise RuntimeError(
                f"Entitlements do not contain app ID: {self.entitlements}"
            )
        return AppId.from_string(maybe_app_id)

    # See `ProvisioningProfileMetadata::getMergeableEntitlements` from `ProvisioningProfileMetadata.java` in Buck v1
    def get_mergeable_entitlements(self) -> dict[str, Any]:
        return {
            k: v
            for k, v in self.entitlements.items()
            if k in ProvisioningProfileMetadata._mergeable_entitlements_keys
        }

    # See `ProvisioningProfileMetadataFactory::fromProvisioningProfilePath` from `ProvisioningProfileMetadataFactory.java` in Buck v1
    @staticmethod
    def from_provisioning_profile_file_content(
        file_path: Path, content: bytes
    ) -> ProvisioningProfileMetadata:
        root = detect_format_and_loads(content)
        developer_certificate_fingerprints = {
            hashlib.sha1(c).hexdigest().upper() for c in root["DeveloperCertificates"]
        }
        assert len(developer_certificate_fingerprints) > 0, (
            "Expected at least one suitable certificate."
        )
        return ProvisioningProfileMetadata(
            file_path=file_path,
            uuid=root["UUID"],
            expiration_date=root["ExpirationDate"],
            platforms=frozenset(root["Platform"]),
            developer_certificate_fingerprints=frozenset(
                developer_certificate_fingerprints
            ),
            entitlements=root["Entitlements"],
            creation_date=root.get("CreationDate"),
            provisions_all_devices=root.get("ProvisionsAllDevices"),
            provisioned_devices=root.get("ProvisionedDevices"),
        )

    def __hash__(self) -> int:
        return hash(
            (
                self.file_path,
                self.uuid,
                self.expiration_date,
                self.platforms,
                self.developer_certificate_fingerprints,
            )
        )

    def to_dict(self) -> dict[str, object]:
        return {
            "file_path": str(self.file_path),
            "uuid": self.uuid,
            "expiration_date": _serialize_datetime(self.expiration_date),
            "creation_date": _serialize_datetime(self.creation_date),
            "platforms": sorted(self.platforms),
            "developer_certificate_fingerprints": sorted(
                self.developer_certificate_fingerprints
            ),
            "entitlements": _make_entitlements_json_safe(self.entitlements),
            "provisions_all_devices": self.provisions_all_devices,
            "provisioned_devices": self.provisioned_devices,
        }

    @staticmethod
    def from_dict(value: object) -> ProvisioningProfileMetadata:
        data = expect_dict(value, "ProvisioningProfileMetadata")
        expect_keys(
            data,
            "ProvisioningProfileMetadata",
            frozenset(
                {
                    "file_path",
                    "uuid",
                    "expiration_date",
                    "creation_date",
                    "platforms",
                    "developer_certificate_fingerprints",
                    "entitlements",
                    "provisions_all_devices",
                    "provisioned_devices",
                }
            ),
        )
        expiration = _deserialize_datetime(
            data["expiration_date"], "ProvisioningProfileMetadata.expiration_date"
        )
        if expiration is None:
            raise ValueError(
                "ProvisioningProfileMetadata.expiration_date must be a string"
            )
        creation = _deserialize_datetime(
            data["creation_date"], "ProvisioningProfileMetadata.creation_date"
        )
        entitlements = _restore_entitlements_from_json_safe(data["entitlements"])
        if not isinstance(entitlements, dict):
            raise ValueError(
                "ProvisioningProfileMetadata.entitlements must be an object"
            )
        return ProvisioningProfileMetadata(
            file_path=Path(
                expect_str(data["file_path"], "ProvisioningProfileMetadata.file_path")
            ),
            uuid=expect_str(data["uuid"], "ProvisioningProfileMetadata.uuid"),
            expiration_date=expiration,
            platforms=frozenset(
                expect_str_list(
                    data["platforms"], "ProvisioningProfileMetadata.platforms"
                )
            ),
            developer_certificate_fingerprints=frozenset(
                expect_str_list(
                    data["developer_certificate_fingerprints"],
                    "ProvisioningProfileMetadata.developer_certificate_fingerprints",
                )
            ),
            entitlements=cast(dict[str, Any], entitlements),
            creation_date=creation,
            provisions_all_devices=expect_optional_bool(
                data["provisions_all_devices"],
                "ProvisioningProfileMetadata.provisions_all_devices",
            ),
            provisioned_devices=expect_optional_str_list(
                data["provisioned_devices"],
                "ProvisioningProfileMetadata.provisioned_devices",
            ),
        )
