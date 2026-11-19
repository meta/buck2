# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

# pyre-strict

import pathlib
import sys
from typing import List, Optional

from tap import Tap

from .apple_platform import ApplePlatform
from .codesign_bundle import (
    AdhocSigningContext,
    codesign_bundle,
    CodesignedPath,
    signing_context_with_profile_selection,
)
from .list_codesign_identities import ListCodesignIdentities
from .provisioning_profile_selection import CodeSignProvisioningError


class Arguments(Tap):
    """
    Tool which code signs the Apple bundle. `Info.plist` file is amended as a part of it.
    """

    # pyre-fixme[13]: Attribute `bundle_path` is never initialized.
    bundle_path: pathlib.Path
    # pyre-fixme[13]: Attribute `info_plist` is never initialized.
    info_plist: pathlib.Path
    entitlements: Optional[pathlib.Path] = None
    profiles_dir: Optional[pathlib.Path] = None
    ad_hoc: bool = False
    ad_hoc_codesign_identity: Optional[str] = None
    # pyre-fixme[13]: Attribute `platform` is never initialized.
    platform: ApplePlatform
    codesign_on_copy: Optional[List[pathlib.Path]] = None
    fast_provisioning_profile_parsing: bool = False
    strict_provisioning_profile_search: bool = False
    provisioning_profile_filter: Optional[str] = None
    only_select_provisioning_profile: bool = False
    no_check_certificates: bool = False

    def configure(self) -> None:
        """
        Configure the arguments.
        """
        self.add_argument(
            "--bundle-path",
            metavar="</path/to/app.bundle>",
            type=pathlib.Path,
            required=True,
            help="Absolute path to Apple bundle result.",
        )
        self.add_argument(
            "--info-plist",
            metavar="<Info.plist>",
            type=pathlib.Path,
            required=True,
            help="Bundle relative destination path to Info.plist file if it is present in bundle.",
        )
        self.add_argument(
            "--entitlements",
            metavar="<Entitlements.plist>",
            type=pathlib.Path,
            required=False,
            help="Path to file with entitlements to be used during code signing. If it's not provided the minimal entitlements are going to be generated.",
        )
        self.add_argument(
            "--profiles-dir",
            metavar="</provisioning/profiles/directory>",
            type=pathlib.Path,
            required=False,
            action="append",
            help="Path to directory with provisioning profile files. Required if code signing is not ad-hoc. Can be specified multiple times.",
        )
        self.add_argument(
            "--ad-hoc",
            action="store_true",
            required=False,
            help="Perform ad-hoc signing if set.",
        )
        self.add_argument(
            "--ad-hoc-codesign-identity",
            metavar="<identity>",
            type=str,
            required=False,
            help="Codesign identity to use when ad-hoc signing is performed.",
        )
        self.add_argument(
            "--platform",
            metavar="<apple platform>",
            type=ApplePlatform,
            required=True,
            help="Apple platform for which the bundle was built.",
        )
        self.add_argument(
            "--codesign-on-copy",
            metavar="<codesign/this/path>",
            type=pathlib.Path,
            action="append",
            required=False,
            help="Bundle relative path that should be codesigned prior to result bundle.",
        )
        self.add_argument(
            "--fast-provisioning-profile-parsing",
            action="store_true",
            required=False,
            help="Uses experimental faster provisioning profile parsing.",
        )
        self.add_argument(
            "--strict-provisioning-profile-search",
            action="store_true",
            required=False,
            help="Fail code signing if more than one matching profile found.",
        )
        self.add_argument(
            "--provisioning-profile-filter",
            metavar="<regex>",
            type=str,
            required=False,
            help="Regex to disambiguate multiple matching profiles, evaluated against provisioning profile filename.",
        )
        self.add_argument(
            "--only-select-provisioning-profile",
            action="store_true",
            required=False,
            help="Skip codesigning and just output the path to the selected provisioning profile to stdout.",
        )
        self.add_argument(
            "--no-check-certificates",
            action="store_true",
            required=False,
            help="Skip the check on code signing identities when selecting provisioning profile.",
        )


# Add emoji to beginning of actionable error message so it stands out more.
def decorate_error_message(message: str) -> str:
    return " ".join(["❗️", message])


def _main() -> None:
    args = Arguments().parse_args()
    try:
        if args.ad_hoc:
            if args.only_select_provisioning_profile:
                print(
                    "No provisioning profile is selected for ad-hoc signing.",
                    file=sys.stderr,
                )
                exit(1)
            signing_context = AdhocSigningContext(
                codesign_identity=args.ad_hoc_codesign_identity
            )
        else:
            assert args.profiles_dir, (
                "Path to directory with provisioning profile files should be set when signing is not ad-hoc."
            )
            signing_context = signing_context_with_profile_selection(
                info_plist_source=args.bundle_path / args.info_plist,
                info_plist_destination=args.info_plist,
                provisioning_profiles_dirs=args.profiles_dir,
                entitlements_path=args.entitlements,
                list_codesign_identities=ListCodesignIdentities.default(),
                platform=args.platform,
                should_use_fast_provisioning_profile_parsing=args.fast_provisioning_profile_parsing,
                strict_provisioning_profile_search=args.strict_provisioning_profile_search,
                provisioning_profile_filter=args.provisioning_profile_filter,
                no_check_certificates=args.no_check_certificates,
            )

            if args.only_select_provisioning_profile:
                print(signing_context.selected_profile_info.profile.file_path)
                return

        bundle_path = CodesignedPath(
            path=args.bundle_path,
            entitlements=args.entitlements,
            flags=[],
            extra_file_paths=None,
        )
        codesign_on_copy_paths = (
            [
                CodesignedPath(
                    path=bundle_path.path / path,
                    entitlements=None,
                    flags=[],
                    extra_file_paths=None,
                )
                for path in args.codesign_on_copy
            ]
            if args.codesign_on_copy
            else []
        )

        codesign_bundle(
            bundle_path=bundle_path,
            signing_context=signing_context,
            platform=args.platform,
            codesign_on_copy_paths=codesign_on_copy_paths,
        )
    except CodeSignProvisioningError as e:
        print(decorate_error_message(str(e)), file=sys.stderr)
        exit(1)


if __name__ == "__main__":
    _main()
