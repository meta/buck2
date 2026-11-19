/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io;
use std::path::Path;

use allocative::Allocative;
use buck2_error::BuckErrorContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::abs_path::AbsPath;
use pagable::Pagable;
use regex::Regex;
use serde::Deserialize;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Input)]
enum XcodeVersionError {
    #[error("XCode select symlink `{}` resolved to path without parent: `{}`", XCODE_SELECT_SYMLINK, _0.display())]
    XcodeSelectSymlinkResolvedToPathWithoutParent(AbsNormPathBuf),
    #[error("Expected short version `{0}` to contain at least major and minor versions")]
    MalformedShortVersion(String),
    #[error("Expected valid format for 'version-build' (e.g., 14.3.0-14C18 or 14.1-14B47b)")]
    MalformedVersionBuildString,
}

const XCODE_SELECT_SYMLINK: &str = "/var/db/xcode_select_link";

/// Only fields we care about from Xcode version.plist.
#[derive(Deserialize)]
#[allow(non_snake_case)]
struct XcodeVersionPlistSchema {
    CFBundleShortVersionString: String,
    ProductBuildVersion: String,
}

/// Versioning information for the currently selected Xcode on the host machine.
#[derive(Debug, Default, PartialEq, Clone, Allocative, Pagable)]
pub struct XcodeVersionInfo {
    /// e.g. "14.0.1"
    pub version_string: String,
    /// The "14" in "14.0.1"
    pub major_version: String,
    /// The "0" in "14.0.1"
    pub minor_version: String,
    /// The "1" in "14.0.1"
    pub patch_version: String,
    /// Xcode-specific build number like "14A309"
    pub build_number: String,
}

impl XcodeVersionInfo {
    // Construct from version.plist in root of Xcode install dir.
    pub fn new() -> buck2_error::Result<Option<Self>> {
        let resolved_xcode_path =
            fs_util::canonicalize_if_exists(AbsPath::new(XCODE_SELECT_SYMLINK)?)
                .buck_error_context("resolve selected xcode link")?;
        let resolved_xcode_path = match resolved_xcode_path {
            Some(p) => p,
            None => return Ok(None),
        };
        let plist_parent_path = resolved_xcode_path.parent().ok_or_else(|| {
            XcodeVersionError::XcodeSelectSymlinkResolvedToPathWithoutParent(
                resolved_xcode_path.clone(),
            )
        })?;
        let plist_path = plist_parent_path.as_path().join("version.plist");
        Self::from_plist(&plist_path)
    }

    pub(crate) fn from_plist(plist_path: &Path) -> buck2_error::Result<Option<Self>> {
        let plist = plist::from_file::<_, XcodeVersionPlistSchema>(plist_path);

        let plist = match plist {
            Ok(plist) => plist,
            Err(e)
                if e.as_io()
                    .is_some_and(|e| e.kind() == io::ErrorKind::NotFound) =>
            {
                return Ok(None);
            }
            Err(e) => {
                return Err(from_any_with_tag(e, buck2_error::ErrorTag::Tier0)
                    .context("Error deserializing Xcode `version.plist`"));
            }
        };

        let version_parts = &mut plist.CFBundleShortVersionString.split('.');
        let major = version_parts
            .next()
            .ok_or_else(|| {
                XcodeVersionError::MalformedShortVersion(plist.CFBundleShortVersionString.clone())
            })
            .map(|v| v.to_owned())?;
        let minor = version_parts.next().unwrap_or("0").to_owned();
        let patch = version_parts.next().unwrap_or("0").to_owned();
        let build_number = plist.ProductBuildVersion;

        Ok(Some(Self {
            version_string: plist.CFBundleShortVersionString,
            major_version: major,
            minor_version: minor,
            patch_version: patch,
            build_number,
        }))
    }

    /// Construct from a string, formatted as: "version-build"
    /// (e.g., 14.3.0-14C18 or 14.1-14B47b)
    pub fn from_version_and_build(version_and_build: &str) -> buck2_error::Result<Self> {
        let re = Regex::new(r"^((\d+)\.(\d+)(?:\.(\d+))?)\-([[:alnum:]]+)$").unwrap();
        if !re.is_match(version_and_build) {
            return Err(XcodeVersionError::MalformedVersionBuildString.into());
        }

        let caps = re.captures(version_and_build).unwrap();
        let version = caps.get(1).map_or("", |m| m.as_str());
        let major = caps.get(2).map_or("", |m| m.as_str());
        let minor = caps.get(3).map_or("", |m| m.as_str());
        let patch = caps.get(4).map_or("0", |m| m.as_str());
        let build = caps.get(5).map_or("", |m| m.as_str());

        Ok(Self {
            version_string: version.to_owned(),
            major_version: major.to_owned(),
            minor_version: minor.to_owned(),
            patch_version: patch.to_owned(),
            build_number: build.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    fn write_plist(plist_content: &str) -> (tempfile::TempDir, PathBuf) {
        let workspace = tempfile::tempdir().expect("failed to create tempdir");
        let fake_xcode_dir = workspace.path().join("xcode_foo_bar.app").join("Contents");
        fs::DirBuilder::new()
            .recursive(true)
            .create(&fake_xcode_dir)
            .unwrap();
        let plist_path = fake_xcode_dir.join("version.plist");
        fs::write(&plist_path, plist_content).expect("failed to write plist");
        (workspace, plist_path)
    }

    #[test]
    fn test_resolves_version_from_plist() {
        let (_t, plist) = write_plist(
            r#"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
        <key>BuildVersion</key>
        <string>2</string>
        <key>CFBundleShortVersionString</key>
        <string>14.1.2</string>
        <key>CFBundleVersion</key>
        <string>21534.1</string>
        <key>ProductBuildVersion</key>
        <string>14B47b</string>
        <key>ProjectName</key>
        <string>IDEFrameworks</string>
        <key>SourceVersion</key>
        <string>21534001000000000</string>
</dict>
</plist>
        "#,
        );

        let got = XcodeVersionInfo::from_plist(&plist)
            .expect("failed to parse version info")
            .unwrap();
        let want = XcodeVersionInfo {
            version_string: "14.1.2".to_owned(),
            major_version: "14".to_owned(),
            minor_version: "1".to_owned(),
            patch_version: "2".to_owned(),
            build_number: "14B47b".to_owned(),
        };
        assert_eq!(want, got);
    }

    #[test]
    fn test_resolves_version_from_plist_no_patch_version() {
        let (_t, plist) = write_plist(
            r#"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
        <key>BuildVersion</key>
        <string>2</string>
        <key>CFBundleShortVersionString</key>
        <string>14.1</string>
        <key>CFBundleVersion</key>
        <string>21534.1</string>
        <key>ProductBuildVersion</key>
        <string>14B47b</string>
        <key>ProjectName</key>
        <string>IDEFrameworks</string>
        <key>SourceVersion</key>
        <string>21534001000000000</string>
</dict>
</plist>
        "#,
        );

        let got = XcodeVersionInfo::from_plist(&plist)
            .expect("failed to parse version info")
            .unwrap();
        let want = XcodeVersionInfo {
            version_string: "14.1".to_owned(),
            major_version: "14".to_owned(),
            minor_version: "1".to_owned(),
            patch_version: "0".to_owned(),
            build_number: "14B47b".to_owned(),
        };
        assert_eq!(want, got);
    }

    #[test]
    fn test_resolves_version_from_plist_no_minor_no_patch_version() {
        let (_t, plist) = write_plist(
            r#"
            <?xml version="1.0" encoding="UTF-8"?>
            <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
            <plist version="1.0">
            <dict>
                <key>BuildVersion</key>
                <string>35</string>
                <key>CFBundleShortVersionString</key>
                <string>14</string>
                <key>CFBundleVersion</key>
                <string>21335</string>
                <key>ProductBuildVersion</key>
                <string>14A309</string>
                <key>ProjectName</key>
                <string>IDEFrameworks</string>
                <key>SourceVersion</key>
                <string>21335000000000000</string>
            </dict>
            </plist>
        "#,
        );

        let got = XcodeVersionInfo::from_plist(&plist)
            .expect("failed to parse version info")
            .unwrap();
        let want = XcodeVersionInfo {
            version_string: "14".to_owned(),
            major_version: "14".to_owned(),
            minor_version: "0".to_owned(),
            patch_version: "0".to_owned(),
            build_number: "14A309".to_owned(),
        };
        assert_eq!(want, got);
    }

    #[test]
    fn test_no_plist() {
        let workspace = tempfile::tempdir().expect("failed to create tempdir");
        assert!(
            XcodeVersionInfo::from_plist(&workspace.path().join("foo.plist"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_resolves_version_from_version_and_build_string() {
        let version_build = "14.3.1-14C18";
        let got = XcodeVersionInfo::from_version_and_build(version_build)
            .expect("failed to parse version info");
        let want = XcodeVersionInfo {
            version_string: "14.3.1".to_owned(),
            major_version: "14".to_owned(),
            minor_version: "3".to_owned(),
            patch_version: "1".to_owned(),
            build_number: "14C18".to_owned(),
        };
        assert_eq!(want, got);

        let version_build2 = "14.1-14B47b";
        let got2 = XcodeVersionInfo::from_version_and_build(version_build2)
            .expect("failed to parse version info");
        let want2 = XcodeVersionInfo {
            version_string: "14.1".to_owned(),
            major_version: "14".to_owned(),
            minor_version: "1".to_owned(),
            patch_version: "0".to_owned(),
            build_number: "14B47b".to_owned(),
        };
        assert_eq!(want2, got2);
    }
}
