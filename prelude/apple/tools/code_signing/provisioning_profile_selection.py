# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

from __future__ import annotations

import datetime
import logging
import re
from collections import defaultdict
from dataclasses import dataclass
from typing import Any, cast, Optional

from .app_id import AppId
from .apple_platform import ApplePlatform
from .identity import CodeSigningIdentity
from .info_plist_metadata import InfoPlistMetadata
from .provisioning_profile_diagnostics import (
    BundleIdMismatch,
    DeveloperCertificateMismatch,
    EntitlementsMismatch,
    IProvisioningProfileDiagnostics,
    ProfileExpired,
    TeamIdMismatch,
    UnsupportedPlatform,
)
from .provisioning_profile_metadata import ProvisioningProfileMetadata
from .serialization import expect_dict, expect_keys

_LOGGER: logging.Logger = logging.getLogger(__name__)


class CodeSignProvisioningError(Exception):
    pass


def _parse_team_id_from_entitlements(
    entitlements: Optional[dict[str, Any]],
) -> Optional[str]:
    if not entitlements:
        return None
    maybe_app_id = AppId.infer_from_entitlements(entitlements)
    if not maybe_app_id:
        return None
    return maybe_app_id.team_id


def _matches_or_array_is_subset_of(
    entitlement_name: str,
    expected_value: object,
    actual_value: object,
    platform: ApplePlatform,
) -> bool:
    if expected_value is None:
        return actual_value is None
    if actual_value == "*":
        # Certain entitlements have a wildcard value, which means any value is allowed.
        return True
    if (
        actual_value is None
        and platform.is_desktop()
        and entitlement_name.startswith("com.apple.security")
    ):
        # For macOS apps, including Catalyst, the provisioning profile would _not_ have entries for
        # the sandbox entitlements, so any value matches.
        return True
    if isinstance(expected_value, list) and isinstance(actual_value, list):
        return set(expected_value).issubset(set(actual_value))
    if (
        isinstance(expected_value, str)
        and isinstance(actual_value, list)
        and all(isinstance(value, str) for value in actual_value)
    ):
        return expected_value in actual_value
    return actual_value == expected_value


def _bundle_match_length(expected_bundle_id: str, bundle_id_pattern: str) -> int:
    if bundle_id_pattern.endswith("*"):
        # Chop the ending * if wildcard.
        bundle_id_without_wildcard = bundle_id_pattern[:-1]
        if expected_bundle_id.startswith(bundle_id_without_wildcard):
            return len(bundle_id_without_wildcard)
    elif expected_bundle_id == bundle_id_pattern:
        return len(bundle_id_pattern)
    return -1


# For those keys let the tooling decide if code signing should fail or succeed (every other key
# mismatch results in provisioning profile being skipped).
_IGNORE_MISMATCH_ENTITLEMENTS_KEYS = {
    "keychain-access-groups",
    "application-identifier",
    "com.apple.developer.associated-domains",
    "com.apple.developer.icloud-container-development-container-identifiers",
    "com.apple.developer.icloud-container-environment",
    "com.apple.developer.icloud-container-identifiers",
    "com.apple.developer.icloud-services",
    "com.apple.developer.ubiquity-container-identifiers",
    "com.apple.developer.ubiquity-kvstore-identifier",
}


def _check_entitlements_match(
    expected_entitlements: Optional[dict[str, Any]],
    profile: ProvisioningProfileMetadata,
    platform: ApplePlatform,
    bundle_id_match_length: int,
) -> tuple[bool, Optional[EntitlementsMismatch]]:
    if expected_entitlements is None:
        return (True, None)
    for key, value in expected_entitlements.items():
        profile_entitlement = profile.entitlements.get(key)
        if (key not in _IGNORE_MISMATCH_ENTITLEMENTS_KEYS) and (
            not _matches_or_array_is_subset_of(
                key, value, profile_entitlement, platform
            )
        ):
            return (
                False,
                EntitlementsMismatch(
                    profile=profile,
                    bundle_id_match_length=bundle_id_match_length,
                    mismatched_key=key,
                    mismatched_value=value,
                ),
            )
    return (True, None)


def _check_developer_certificates_match(
    profile: ProvisioningProfileMetadata,
    identities: list[CodeSigningIdentity],
    bundle_id_match_length: int,
) -> tuple[Optional[CodeSigningIdentity], Optional[DeveloperCertificateMismatch]]:
    for identity in identities:
        if identity.fingerprint in profile.developer_certificate_fingerprints:
            return (identity, None)
    return (
        None,
        DeveloperCertificateMismatch(
            profile=profile, bundle_id_match_length=bundle_id_match_length
        ),
    )


def _make_multiple_matching_profiles_message(
    profiles: list[ProvisioningProfileMetadata],
    strict_search: bool,
) -> str:
    messages = [f"Found MULTIPLE matching profiles: {len(profiles)}"]
    messages += [
        f"    Matching Profile = UUID:{profile.uuid}, creation date: {profile.creation_date}, file path: {profile.file_path}"
        for profile in profiles
    ]

    if strict_search:
        messages += [
            "Strict provisioning profile search is ENABLED, build will FAIL due to ambiguous provisioning profile search results.",
            "To resolve the problem, ensure only a single profile matches.",
            "To unblock, you have two options:",
            "Option 1: Disable strict provisioning profile search for the targets failing to build.",
            "    If the target failing to build is an `apple_bundle()`, set the `strict_provisioning_profile_search` attribute to `False`.",
            "    If the target failing to build is produced by `ios_binary()`, set the `bundle_strict_provisioning_profile_search` attribute to `False`.",
            "    You can commit such a change, so that the issue can be investigated without blocking other developers.",
            "    NB: This is a TEMPORARY WORKAROUND, as it only disables the strict checking, it does not resolve the ambiguity.",
            "Option 2: Pass `--config apple.strict_provisioning_profile_search=false` as part of your build command.",
            "    DO NOT COMMIT such a change by adding this to any CI configs.",
        ]

    return "\n".join(messages)


@dataclass
class SelectedProvisioningProfileInfo:
    profile: ProvisioningProfileMetadata
    identity: CodeSigningIdentity

    def to_dict(self) -> dict[str, Any]:
        return {
            "identity": self.identity.to_dict(),
            "profile": self.profile.to_dict(),
        }

    @staticmethod
    def from_dict(value: object) -> SelectedProvisioningProfileInfo:
        data = expect_dict(value, "SelectedProvisioningProfileInfo")
        expect_keys(
            data,
            "SelectedProvisioningProfileInfo",
            frozenset({"identity", "profile"}),
        )
        return SelectedProvisioningProfileInfo(
            profile=ProvisioningProfileMetadata.from_dict(data["profile"]),
            identity=CodeSigningIdentity.from_dict(data["identity"]),
        )


def _filter_matching_selected_provisioning_profile_infos(
    selected_profile_infos: list[SelectedProvisioningProfileInfo],
    provisioning_profile_filter: Optional[str],
) -> list[SelectedProvisioningProfileInfo]:
    if len(selected_profile_infos) <= 1 or (not provisioning_profile_filter):
        return selected_profile_infos

    preference_regex = re.compile(provisioning_profile_filter)
    return [
        matching_profile_info
        for matching_profile_info in selected_profile_infos
        if preference_regex.search(matching_profile_info.profile.file_path.name)
    ]


# See `ProvisioningProfileStore::getBestProvisioningProfile` in `ProvisioningProfileStore.java` for Buck v1 equivalent
# This function operates on data types only without performing any IO or process invocation.
def select_best_provisioning_profile_core(
    info_plist_metadata: InfoPlistMetadata,
    code_signing_identities: list[CodeSigningIdentity],
    provisioning_profiles: list[ProvisioningProfileMetadata],
    entitlements: Optional[dict[str, Any]],
    platform: ApplePlatform,
    strict_search: bool,
    provisioning_profile_filter: Optional[str],
    no_check_certificates: bool = False,
) -> tuple[
    Optional[SelectedProvisioningProfileInfo], list[IProvisioningProfileDiagnostics]
]:
    """Selects the best provisioning profile and certificate to use when code signing the bundle.
       Such profile could be successfully used to sign the bundle taking into account
       different constraints like entitlements or bundle ID. Given several profiles
       could be successfully used to sign the bundle the "best" one is considered
       to be a profile which matches bundle ID  from `Info.plist` the most
       (i.e. profiles with specific bundle ID are preferred to wildcard bundle IDs).
    Args:
       info_plist_metadata: Object representing `Info.plist` file in the bundle.
       code_signing_identities: Code signing identities to choose from.
       provisioning_profiles: Provisioning profiles to choose from.
       entitlements: Code signing entitlements if any.
       platform: Apple platform which the bundle is built for.
    Returns:
       Provisioning profile and certificate pair to use for code signing.
    """
    maybe_team_id_constraint = _parse_team_id_from_entitlements(entitlements)

    best_match_length = -1

    # Used for error messages
    diagnostics: list[IProvisioningProfileDiagnostics] = []

    def log_mismatched_profile(mismatch: IProvisioningProfileDiagnostics) -> None:
        diagnostics.append(mismatch)
        _LOGGER.info(
            f"Skipping provisioning profile `{mismatch.profile.file_path.name}`: {mismatch.log_message()}"
        )

    selected_profile_infos_for_match_length = defaultdict(list)

    for profile in provisioning_profiles:
        app_id = profile.get_app_id()
        bundle_id = app_id.bundle_id
        current_match_length = _bundle_match_length(
            info_plist_metadata.bundle_id, bundle_id
        )
        if current_match_length < 0:
            log_mismatched_profile(
                BundleIdMismatch(
                    profile=profile,
                    bundle_id=app_id.bundle_id,
                    bundle_id_constraint=info_plist_metadata.bundle_id,
                )
            )
            continue

        if maybe_team_id_constraint and maybe_team_id_constraint != app_id.team_id:
            log_mismatched_profile(
                TeamIdMismatch(
                    profile=profile,
                    team_id=app_id.team_id,
                    team_id_constraint=maybe_team_id_constraint,
                )
            )
            continue

        # `ExpirationDate` is read as a naive UTC time, so compare it with UTC, not local time.
        now_utc = datetime.datetime.now(datetime.timezone.utc).replace(tzinfo=None)
        if now_utc > profile.expiration_date:
            log_mismatched_profile(
                ProfileExpired(
                    profile=profile, bundle_id_match_length=current_match_length
                )
            )
            continue

        maybe_provisioning_profile_name = platform.provisioning_profile_name()
        if (
            maybe_provisioning_profile_name
            and maybe_provisioning_profile_name not in profile.platforms
        ):
            log_mismatched_profile(
                UnsupportedPlatform(
                    profile=profile,
                    bundle_id_match_length=current_match_length,
                    platform_constraint=platform,
                )
            )
            continue

        entitlements_matched, mismatch = _check_entitlements_match(
            expected_entitlements=entitlements,
            profile=profile,
            platform=platform,
            bundle_id_match_length=current_match_length,
        )
        if not entitlements_matched:
            log_mismatched_profile(cast(EntitlementsMismatch, mismatch))
            continue

        if no_check_certificates:
            certificate = CodeSigningIdentity(
                fingerprint=next(iter(profile.developer_certificate_fingerprints)),
                subject_common_name="Unknown",
            )
        else:
            certificate, mismatch = _check_developer_certificates_match(
                profile=profile,
                identities=code_signing_identities,
                bundle_id_match_length=current_match_length,
            )
            if not certificate:
                log_mismatched_profile(cast(DeveloperCertificateMismatch, mismatch))
                continue

        _LOGGER.info(
            f"Matching provisioning profile `{profile.file_path.name}` with score {current_match_length}"
        )

        selected_profile_info = SelectedProvisioningProfileInfo(profile, certificate)
        selected_profile_infos_for_match_length[current_match_length] += [
            selected_profile_info
        ]

        if current_match_length > best_match_length:
            best_match_length = current_match_length

    all_matching_selected_profile_infos = selected_profile_infos_for_match_length.get(
        best_match_length, []
    )

    all_matching_selected_profile_infos = (
        _filter_matching_selected_provisioning_profile_infos(
            all_matching_selected_profile_infos, provisioning_profile_filter
        )
    )

    if len(all_matching_selected_profile_infos) > 1:
        all_matching_profiles = [
            selected_profile_info.profile
            for selected_profile_info in all_matching_selected_profile_infos
        ]
        multiple_profiles_message = _make_multiple_matching_profiles_message(
            all_matching_profiles,
            strict_search,
        )
        _LOGGER.info(multiple_profiles_message)
        if strict_search:
            raise CodeSignProvisioningError(multiple_profiles_message)

    result = (
        # If there are multiple matching profiles, pick the one with expiration date furthest in the future
        max(
            all_matching_selected_profile_infos,
            key=lambda profile_info: profile_info.profile.expiration_date,
        )
        if all_matching_selected_profile_infos
        else None
    )

    if result:
        _LOGGER.info(
            f"Found matching provisioning profile and identity\n"
            f"  Selected Identity: {result.identity}\n"
            f"  Provisioning Profile: `{result.profile.file_path.name}`\n"
            f"    UUID: {result.profile.uuid}\n"
            f"    File: {result.profile.file_path}\n"
            f"    Expiration: {result.profile.expiration_date}\n"
            f"    Platforms: {result.profile.platforms}\n"
            f"    Fingerprints: {result.profile.developer_certificate_fingerprints}\n"
            f"    Entitlements: {result.profile.entitlements}"
        )

    return result, diagnostics
