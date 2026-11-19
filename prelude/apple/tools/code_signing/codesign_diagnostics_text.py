# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.


class CodesignDiagnosticsText:
    """Generic remediation text for codesigning diagnostic messages.

    In Meta builds, this class is replaced by the Meta-specific version from
    meta_only/codesign_diagnostics_text.py via @oss-disable/@oss-enable.
    """

    CERT_REMEDIATION: str = "Check that your signing certificate is installed in your keychain and has not expired. Re-download from the Apple Developer Portal if needed."
    ENTITLEMENTS_REMEDIATION: str = "Download and install the latest provisioning profile with the correct entitlements from the Apple Developer Portal."
    PLATFORM_LINK: str = (
        "you would need to download and install it from the Apple Developer Portal"
    )
    PROFILES_DOWNLOAD: str = (
        "Download the latest provisioning profiles from the Apple Developer Portal."
    )
    DEVICE_LINK: str = ""
    NO_PROFILES_REMEDIATION: str = "Download and install the required provisioning profiles from the Apple Developer Portal.\n"
