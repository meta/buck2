# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

from __future__ import annotations

import argparse
import json
import shlex
from pathlib import Path

from apple.tools.code_signing.apple_platform import ApplePlatform
from apple.tools.code_signing.codesign_bundle import (
    signing_context_with_profile_selection,
)
from apple.tools.code_signing.list_codesign_identities import (
    AdHocListCodesignIdentities,
    ListCodesignIdentities,
)
from apple.tools.code_signing.signing_context_types import (
    AdhocSigningContext,
    SigningContextWithProfileSelection,
)


def add_args_for_signing_context_path(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--signing-context-path",
        metavar="<SigningContext.json>",
        type=Path,
        required=True,
        help="Path to the precomputed signing context JSON.",
    )


def add_args_for_bundling_execution(parser: argparse.ArgumentParser) -> None:
    _add_common_execution_args(parser)


def _add_common_execution_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--codesign",
        action="store_true",
        help="Should the final bundle be codesigned.",
    )
    parser.add_argument(
        "--entitlements",
        metavar="<Entitlements.plist>",
        type=Path,
        required=False,
        help="Path to file with entitlements to be used during code signing. If it's not provided the minimal entitlements are going to be generated.",
    )
    parser.add_argument(
        "--entitlements-suffixed-key-map",
        type=json.loads,
        required=False,
        default={},
        help="JSON map of entitlement key to suffix to append to its values.",
    )
    parser.add_argument(
        "--entitlements-removed-keys",
        type=json.loads,
        required=False,
        default=[],
        help="JSON list of entitlement keys to remove from the entitlements.",
    )
    parser.add_argument(
        "--entitlements-removed-values-map",
        type=json.loads,
        required=False,
        default={},
        help="JSON map of entitlement keys to list of values to remove from that key.",
    )
    parser.add_argument(
        "--info-plist-source",
        metavar="</prepared/Info.plist>",
        type=Path,
        required=False,
        help="Path to Info.plist source file which is used only to make code signing decisions (to be bundled `Info.plist` should be present in spec parameter). Required if code signing is requested.",
    )
    parser.add_argument(
        "--info-plist-destination",
        metavar="<Info.plist>",
        type=Path,
        required=False,
        help="Required if code signing is requested. Bundle relative destination path to Info.plist file if it is present in bundle.",
    )
    parser.add_argument(
        "--platform",
        metavar="<apple platform>",
        type=ApplePlatform,
        required=False,
        help="Required if code signing or Swift support is requested. Apple platform for which the bundle is built.",
    )
    parser.add_argument(
        "--log-file",
        type=Path,
        required=False,
        help="Path to a log file. If present logging will be directed to this file in addition to stderr.",
    )


def add_args_for_signing_context_selection(
    parser: argparse.ArgumentParser,
) -> None:
    parser.add_argument(
        "--ad-hoc",
        action="store_true",
        help="Perform ad-hoc signing if set.",
    )
    parser.add_argument(
        "--ad-hoc-codesign-identity",
        metavar="<identity>",
        type=str,
        required=False,
        help="Codesign identity to use when ad-hoc signing is performed. Should be present when selection of provisioining profile is requested for ad-hoc signing.",
    )
    parser.add_argument(
        "--codesign-identities-command",
        metavar='<"/signing/identities --available">',
        type=str,
        required=False,
        help="Command listing available code signing identities. If it's not provided `security` utility is assumed to be available and is used.",
    )
    parser.add_argument(
        "--profiles-dir",
        metavar="</provisioning/profiles/directory>",
        type=Path,
        required=False,
        action="append",
        help="Required if non-ad-hoc code signing is requested. Path to directory with provisioning profile files. Can be specified multiple times.",
    )
    parser.add_argument(
        "--embed-provisioning-profile-when-signing-ad-hoc",
        action="store_true",
        help="Perform selection of provisioining profile and embed it into final bundle when ad-hoc signing if set.",
    )
    parser.add_argument(
        "--fast-provisioning-profile-parsing",
        action="store_true",
        help="Uses experimental faster provisioning profile parsing.",
    )
    parser.add_argument(
        "--strict-provisioning-profile-search",
        action="store_true",
        required=False,
        help="Fail code signing if more than one matching profile found.",
    )
    parser.add_argument(
        "--provisioning-profile-filter",
        metavar="<regex>",
        type=str,
        required=False,
        help="Regex to disambiguate multiple matching profiles, evaluated against provisioning profile filename.",
    )
    parser.add_argument(
        "--verify-entitlements",
        action="store_true",
        help="Verify that the entitlements match the provisioning profile.",
    )
    parser.add_argument(
        "--no-check-certificates",
        action="store_true",
        required=False,
        help="Skip the check on code signing identities when selecting provisioning profile.",
    )


def add_args_for_signing_context(parser: argparse.ArgumentParser) -> None:
    _add_common_execution_args(parser)
    add_args_for_signing_context_selection(parser)


def signing_context_and_selected_identity_from_args(
    args: argparse.Namespace,
) -> tuple[AdhocSigningContext | SigningContextWithProfileSelection | None, str | None]:
    if args.codesign:
        if not args.info_plist_source:
            raise RuntimeError(
                "Paths to Info.plist source file should be set when code signing is required."
            )
        if not args.info_plist_destination:
            raise RuntimeError(
                "Info.plist destination path should be set when code signing is required."
            )
        if not args.platform:
            raise RuntimeError(
                "Apple platform should be set when code signing is required."
            )
        list_codesign_identities = (
            ListCodesignIdentities.override(
                shlex.split(args.codesign_identities_command)
            )
            if args.codesign_identities_command
            else ListCodesignIdentities.default()
        )
        if args.ad_hoc:
            if args.embed_provisioning_profile_when_signing_ad_hoc:
                if not args.profiles_dir:
                    raise RuntimeError(
                        "Path to directory with provisioning profile files should be set when selection of provisioining profile is enabled for ad-hoc code signing."
                    )
                if not args.ad_hoc_codesign_identity:
                    raise RuntimeError(
                        "Code signing identity should be set when selection of provisioining profile is enabled for ad-hoc code signing."
                    )
                profile_selection_context = signing_context_with_profile_selection(
                    info_plist_source=args.info_plist_source,
                    info_plist_destination=args.info_plist_destination,
                    provisioning_profiles_dirs=args.profiles_dir,
                    entitlements_path=args.entitlements,
                    platform=args.platform,
                    list_codesign_identities=AdHocListCodesignIdentities(
                        original=list_codesign_identities,
                        subject_common_name=args.ad_hoc_codesign_identity,
                    ),
                    log_file_path=args.log_file,
                    should_use_fast_provisioning_profile_parsing=args.fast_provisioning_profile_parsing,
                    strict_provisioning_profile_search=args.strict_provisioning_profile_search,
                    provisioning_profile_filter=args.provisioning_profile_filter,
                    should_verify_entitlements=args.verify_entitlements,
                    no_check_certificates=args.no_check_certificates,
                )
            else:
                profile_selection_context = None
            signing_context = AdhocSigningContext(
                codesign_identity=args.ad_hoc_codesign_identity,
                profile_selection_context=profile_selection_context,
            )
            selected_identity_argument = args.ad_hoc_codesign_identity
        else:
            if not args.profiles_dir:
                raise RuntimeError(
                    "Path to directory with provisioning profile files should be set when signing is not ad-hoc."
                )
            signing_context = signing_context_with_profile_selection(
                info_plist_source=args.info_plist_source,
                info_plist_destination=args.info_plist_destination,
                provisioning_profiles_dirs=args.profiles_dir,
                entitlements_path=args.entitlements,
                platform=args.platform,
                list_codesign_identities=list_codesign_identities,
                log_file_path=args.log_file,
                should_use_fast_provisioning_profile_parsing=args.fast_provisioning_profile_parsing,
                strict_provisioning_profile_search=args.strict_provisioning_profile_search,
                provisioning_profile_filter=args.provisioning_profile_filter,
                should_verify_entitlements=args.verify_entitlements,
                no_check_certificates=args.no_check_certificates,
            )
            selected_identity_argument = (
                signing_context.selected_profile_info.identity.fingerprint
            )
    else:
        signing_context = None
        selected_identity_argument = None

    return signing_context, selected_identity_argument
