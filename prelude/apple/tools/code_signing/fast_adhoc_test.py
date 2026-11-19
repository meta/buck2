# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is dual-licensed under either the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree or the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree. You may select, at your option, one of the
# above-listed licenses.

import plistlib
import tempfile
import unittest
from pathlib import Path

from .apple_platform import ApplePlatform
from .fast_adhoc import _find_executable_for_signed_path


class FindExecutableForSignedPathTest(unittest.TestCase):
    def test_bundle_without_executable_returns_none(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            app = Path(directory) / "Application.app"
            app.mkdir()
            with open(app / "Info.plist", "wb") as file:
                plistlib.dump({}, file)

            self.assertIsNone(
                _find_executable_for_signed_path(app, ApplePlatform.ios_simulator)
            )

    def test_bundle_with_executable_returns_binary_path(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            app = Path(directory) / "Application.app"
            app.mkdir()
            with open(app / "Info.plist", "wb") as file:
                plistlib.dump({"CFBundleExecutable": "Application"}, file)

            self.assertEqual(
                app / "Application",
                _find_executable_for_signed_path(app, ApplePlatform.ios_simulator),
            )

    def test_versioned_macos_framework_returns_none(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            framework = Path(directory) / "Sparkle.framework"
            resources = framework / "Versions" / "A" / "Resources"
            resources.mkdir(parents=True)
            with open(resources / "Info.plist", "wb") as file:
                plistlib.dump({"CFBundleExecutable": "Sparkle"}, file)

            self.assertIsNone(
                _find_executable_for_signed_path(framework, ApplePlatform.macos)
            )
