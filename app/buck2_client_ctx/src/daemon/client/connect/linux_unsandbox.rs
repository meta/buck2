/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::fmt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::LazyLock;

use buck2_core::soft_error;
use buck2_error::BuckErrorContext;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_error::internal_error;
use buck2_fs::async_fs_util;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_util::process::async_background_command;
use regex::Regex;

use super::BuckdConnectDaemonOptions;
use super::ExecutableAndArgs;

// `YYYYMMDD-HHMMSS` release strings sort lexicographically in the same order as their timestamps.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct BuckWrapperRelease<'a>(Cow<'a, str>);

impl BuckWrapperRelease<'_> {
    fn as_str(&self) -> &str {
        self.0.as_ref()
    }

    async fn create(wrapper: &str) -> Option<BuckWrapperRelease<'static>> {
        let output = async_background_command(wrapper)
            .arg("--version-wrapper")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .ok()?;

        if !output.status.success() {
            tracing::debug!(
                "`{} --version-wrapper` failed: {}",
                wrapper,
                String::from_utf8_lossy(&output.stderr)
            );
            return None;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        BuckWrapperRelease::parse_version_wrapper_output(&stdout)
            .map(|release| BuckWrapperRelease(Cow::Owned(release.as_str().to_owned())))
    }

    fn is_at_least(&self, min_release: &BuckWrapperRelease<'_>) -> bool {
        self.as_str() >= min_release.as_str()
    }
}

impl<'a> BuckWrapperRelease<'a> {
    fn parse_version_wrapper_output(output: &'a str) -> Option<BuckWrapperRelease<'a>> {
        static BUCK_WRAPPER_RELEASE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?m)^Release:\s*([0-9]{8}-[0-9]{6})(?:\s|$)")
                .expect("hardcoded regex should compile")
        });

        BUCK_WRAPPER_RELEASE.captures(output).and_then(|captures| {
            captures
                .get(1)
                .map(|release| BuckWrapperRelease(Cow::Borrowed(release.as_str())))
        })
    }
}

impl fmt::Display for BuckWrapperRelease<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

pub(super) async fn get_unix_daemon_and_args<'a>(
    options: &BuckdConnectDaemonOptions,
    args: Vec<&'a str>,
) -> buck2_error::Result<ExecutableAndArgs<'a>> {
    let daemon_exe = super::get_daemon_exe()?;

    if let Some((unsandboxed_daemon_wrapper, canonical_daemon_exe)) =
        daemon_unsandboxed_wrapper(options, &daemon_exe).await?
    {
        let args = std::iter::once(Cow::Borrowed("unsandbox-daemon"))
            .chain(std::iter::once(Cow::Owned(canonical_daemon_exe)))
            .chain(args.into_iter().map(Cow::Borrowed))
            .collect();
        return Ok(ExecutableAndArgs {
            executable: unsandboxed_daemon_wrapper.into(),
            args,
        });
    }

    Ok(ExecutableAndArgs {
        executable: daemon_exe.into_os_string(),
        args: args.into_iter().map(Cow::Borrowed).collect(),
    })
}

async fn daemon_unsandboxed_wrapper(
    options: &BuckdConnectDaemonOptions,
    daemon_exe: &Path,
) -> buck2_error::Result<Option<(&'static str, String)>> {
    const UNSANDBOX_DAEMON_WRAPPER: &str = "/usr/local/bin/buck";
    // This was the first release (d242c0c75785ff34bfcefbecc9cf4a40022df37b) that accepted the `unsandbox-daemon` command.
    const MIN_UNSANDBOX_DAEMON_WRAPPER_RELEASE: BuckWrapperRelease<'static> =
        BuckWrapperRelease(Cow::Borrowed("20260630-090501"));

    if !options.allow_daemon_start_unsandboxed_via_wrapper {
        return Ok(None);
    }

    if identity_env::agent_identity_from_env().is_none() {
        return Ok(None);
    }

    // TODO(jtbraun): Once all old wrappers are gone, we can remove most of these safety checks on the wrapper's suitability.
    const ROOT_UID: u32 = 0;
    const SETUID_MODE: u32 = 0o4000;
    let wrapper_is_setuid = match AbsPath::new(Path::new(UNSANDBOX_DAEMON_WRAPPER)) {
        Ok(wrapper) => async_fs_util::metadata(wrapper)
            .await
            .is_ok_and(|metadata| {
                metadata.uid() == ROOT_UID && metadata.permissions().mode() & SETUID_MODE != 0
            }),
        Err(_) => false,
    };
    if !wrapper_is_setuid {
        soft_error!(
            "buck_unsandbox_daemon_wrapper_not_setuid",
            buck2_error!(
                ErrorTag::Environment,
                "Not starting daemon via `{}` because it is not setuid",
                UNSANDBOX_DAEMON_WRAPPER
            ),
            task: false
        );
        return Ok(None);
    }

    let Some(release) = BuckWrapperRelease::create(UNSANDBOX_DAEMON_WRAPPER).await else {
        soft_error!(
            "buck_unsandbox_daemon_wrapper_release_unavailable",
            buck2_error!(
                ErrorTag::Environment,
                "Not starting daemon via `{}` because its wrapper release could not be fetched",
                UNSANDBOX_DAEMON_WRAPPER
            ),
            task: false
        );
        return Ok(None);
    };

    if !release.is_at_least(&MIN_UNSANDBOX_DAEMON_WRAPPER_RELEASE) {
        soft_error!(
            "buck_unsandbox_daemon_wrapper_release_too_old",
            buck2_error!(
                ErrorTag::Environment,
                "Not starting daemon via `{}` because wrapper release `{}` is older than `{}`",
                UNSANDBOX_DAEMON_WRAPPER,
                release,
                MIN_UNSANDBOX_DAEMON_WRAPPER_RELEASE
            ),
            task: false
        );
        return Ok(None);
    }

    let canonical_daemon_exe = daemon_exe
        .canonicalize()
        .buck_error_context("Failed to canonicalize Buck daemon executable")?;
    if !matches!(
        canonical_daemon_exe
            .file_name()
            .and_then(|filename| filename.to_str()),
        Some("buck2" | "buck2-daemon")
    ) {
        let daemon_filename = canonical_daemon_exe
            .file_name()
            .map(|filename| filename.to_string_lossy())
            .unwrap_or(Cow::Borrowed("<none>"));
        soft_error!(
            "buck_unsandbox_daemon_unsupported_daemon_filename",
            buck2_error!(
                ErrorTag::Environment,
                "Skipping Buck daemon sandbox escape because daemon executable filename `{}` is not supported by `{}`; expected `buck2` or `buck2-daemon`",
                daemon_filename,
                UNSANDBOX_DAEMON_WRAPPER
            ),
            task: false
        );
        return Ok(None);
    }

    let canonical_daemon_exe = canonical_daemon_exe
        .into_os_string()
        .into_string()
        .map_err(|_| internal_error!("Buck daemon executable path is not valid UTF-8"))?;

    Ok(Some((UNSANDBOX_DAEMON_WRAPPER, canonical_daemon_exe)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version_wrapper_output() {
        assert_eq!(
            BuckWrapperRelease::parse_version_wrapper_output(
                "Revision: d242c0c75785ff34bfcefbecc9cf4a40022df37b\nRelease:  20260630-090501\n",
            ),
            Some(BuckWrapperRelease(Cow::Borrowed("20260630-090501")))
        );
        assert_eq!(
            BuckWrapperRelease::parse_version_wrapper_output("Release:  20260630\n"),
            None
        );
        assert_eq!(
            BuckWrapperRelease::parse_version_wrapper_output("Release:  20260630-abcdef\n"),
            None
        );
    }

    #[test]
    fn test_parse_version_wrapper_output_rejects_error() {
        assert_eq!(
            BuckWrapperRelease::parse_version_wrapper_output(
                "Revision: ERROR. Maybe you are using a local build?\nRelease:  ERROR. Maybe you are using a local build?\n",
            ),
            None
        );
    }

    #[test]
    fn test_is_at_least() {
        const MIN_UNSANDBOX_DAEMON_WRAPPER_RELEASE: BuckWrapperRelease<'static> =
            BuckWrapperRelease(Cow::Borrowed("20260630-090501"));

        assert!(
            BuckWrapperRelease(Cow::Borrowed("20260630-090501"))
                .is_at_least(&MIN_UNSANDBOX_DAEMON_WRAPPER_RELEASE)
        );
        assert!(
            BuckWrapperRelease(Cow::Borrowed("20260701-000000"))
                .is_at_least(&MIN_UNSANDBOX_DAEMON_WRAPPER_RELEASE)
        );
        assert!(
            !BuckWrapperRelease(Cow::Borrowed("20260630-090500"))
                .is_at_least(&MIN_UNSANDBOX_DAEMON_WRAPPER_RELEASE)
        );
    }
}
