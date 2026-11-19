/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::env;
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::sync::LazyLock;

use allocative::Allocative;
use buck2_core::soft_error_or_panic;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::ErrorTag;
use buck2_util::process::async_background_command;
use regex::RegexSet;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Sapling)]
pub(crate) enum SaplingError {
    #[error("Failed to run `hg {subcommand}`")]
    #[buck2(tag = command_error_tag(&error))]
    CommandFailed {
        subcommand: &'static str,
        #[source]
        error: io::Error,
    },

    #[error("Failed to obtain mergebase (exit code {}):\n{stderr}", exit_code_display(*exit_code))]
    #[buck2(tag = sapling_error_tag(&stderr))]
    Mergebase {
        exit_code: Option<i32>,
        stderr: String,
    },

    #[error("Warning while obtaining mergebase:\n{stderr}")]
    #[buck2(tag = sapling_error_tag(&stderr))]
    MergebaseWarning { stderr: String },

    #[error("Failed to read stdout when invoking `hg {subcommand}`")]
    #[buck2(tag = SaplingInvalidOutput)]
    MissingStdout { subcommand: &'static str },

    #[error("Invalid status line: {line}")]
    #[buck2(tag = SaplingInvalidOutput)]
    InvalidStatusLine { line: String },
}

fn exit_code_display(exit_code: Option<i32>) -> String {
    exit_code.map_or_else(|| "unknown".to_owned(), |code| code.to_string())
}

/// Patterns matching the messages Sapling prints for root causes we can distinguish, each with an
/// example of what it matches.
///
/// Exit codes are not usable for this: the great majority of failures exit with 1 or 255
/// regardless of cause. Order is priority order, and matters where one message contains another -
/// the missing binary is reported as an `Os { .. NotFound .. }` error, so it has to come before
/// anything else that looks like a missing file.
const SAPLING_ERROR_SIGNATURES: &[(&str, ErrorTag)] = &[
    // Could not start command "hg.real", is it on your PATH? (error: Os { code: 2, kind: NotFound })
    (
        r#"Could not start command ".*", is it on your PATH\?"#,
        ErrorTag::SaplingNotFound,
    ),
    // [BLOCKED] hg command blocked by Crucible VCS isolation: 'hg log' is not an allowed
    // current-workspace query
    (
        r"\[BLOCKED\] hg command blocked by",
        ErrorTag::BlockedByPolicy,
    ),
    // abort: Transport endpoint is not connected: bookmarks.current
    // Eden is not serving the mount anymore
    (
        "Transport endpoint is not connected",
        ErrorTag::IoNotConnected,
    ),
    // error.TlsError: [35] SSL connect error (OpenSSL/1.1.1zf: error:1408F10B:SSL
    // routines:ssl3_get_record:wrong version number)
    // tls error: [35] SSL connect error (Recv failure: Connection reset by peer)!
    // The second form is what is printed without `--traceback`
    (r"(?i)tls ?error", ErrorTag::Tls),
    // abort: command failed due to network error (see /tmp/hg-network.log for details)
    (
        "command failed due to network error",
        ErrorTag::SaplingNetwork,
    ),
    // error.HttpError: [7] Couldn't connect to server (Failed to connect to
    // mononoke.internal.tfbnw.net port 443 after 8 ms)
    (r"HttpError: \[\d+\]", ErrorTag::SaplingNetwork),
    // abort: error getting current working directory: No such file or directory
    (
        "error getting current working directory",
        ErrorTag::MissingWorkingDir,
    ),
    // abort: repository /data/sandcastle/boxes/trunk-hg-full-fbsource not found!
    (r"repository .* not found!", ErrorTag::MissingRepo),
    // abort: '/tmp/scratch' is not inside a repository, but this command requires a repository!
    ("is not inside a repository", ErrorTag::MissingRepo),
    // abort: cannot initialize working copy: repo /tmp/scratch missing dot dir
    ("missing dot dir", ErrorTag::MissingRepo),
    // abort: Permission denied (os error 13) at path "/data/users/build/fbsource"
    (
        r"Permission denied \(os error",
        ErrorTag::IoPermissionDenied,
    ),
];

static SAPLING_ERROR_SIGNATURE_SET: LazyLock<RegexSet> = LazyLock::new(|| {
    RegexSet::new(SAPLING_ERROR_SIGNATURES.iter().map(|(pattern, _)| pattern))
        .expect("should be valid regexes, they are a constant in this file")
});

/// Classifies a failure to run Sapling at all, which reports no stderr of its own.
fn command_error_tag(error: &io::Error) -> ErrorTag {
    match error.kind() {
        io::ErrorKind::NotFound => ErrorTag::SaplingNotFound,
        io::ErrorKind::PermissionDenied => ErrorTag::IoPermissionDenied,
        _ => ErrorTag::Sapling,
    }
}

/// Classifies a Sapling failure by the root cause it reported on stderr.
fn sapling_error_tag(stderr: &str) -> ErrorTag {
    // Matches are yielded in table order, so the earliest match wins.
    SAPLING_ERROR_SIGNATURE_SET
        .matches(stderr)
        .into_iter()
        .next()
        .map_or(ErrorTag::Sapling, |i| SAPLING_ERROR_SIGNATURES[i].1)
}

#[derive(Debug, PartialEq)]
pub(crate) enum SaplingStatus {
    Modified,
    Added,
    Removed,
    Clean,
    Missing,
    NotTracked,
    Ignored,
    Copied,
}

pub(crate) enum SaplingGetStatusResult {
    Normal(Vec<(SaplingStatus, String)>),
    TooManyChanges,
}

#[allow(dead_code)]
#[derive(Allocative, Clone)]
pub(crate) struct MergebaseDetails {
    pub mergebase: String,
    pub timestamp: Option<u64>,
    pub global_rev: Option<u64>,
}

impl PartialEq for MergebaseDetails {
    fn eq(&self, other: &Self) -> bool {
        self.mergebase == other.mergebase
    }
}

fn get_sapling_exe_path() -> String {
    env::var("EDEN_HG_BINARY").unwrap_or("hg".to_owned())
}

pub(crate) async fn get_mergebase<D: AsRef<Path>, C: AsRef<str>, M: AsRef<str>>(
    current_dir: D,
    commit: C,
    mergegase_with: M,
) -> buck2_error::Result<Option<MergebaseDetails>> {
    let output = async_background_command(get_sapling_exe_path())
        .current_dir(current_dir)
        .env("HGPLAIN", "1")
        .args([
            "log",
            "--traceback",
            "-T",
            "{node}\n{date}\n{get(extras, \"global_rev\")}",
            "-r",
            format!(
                "present(ancestor({}, {}))",
                commit.as_ref(),
                mergegase_with.as_ref()
            )
            .as_str(),
        ])
        .output()
        .await
        .map_err(|error| SaplingError::CommandFailed {
            subcommand: "log",
            error,
        })?;

    if !output.status.success() {
        return Err(SaplingError::Mergebase {
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
        .into());
    } else if !output.stderr.is_empty() {
        // Sapling's stderr from a successful `sl log`. Nobody has characterized what it says in
        // practice, so it is recorded for the oncall and not shown to the user.
        soft_error_or_panic!(
            "sapling_mergebase_warning",
            SaplingError::MergebaseWarning {
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }
            .into()
        );
    }

    parse_log_output(output.stdout)
}

fn parse_log_output(output: Vec<u8>) -> buck2_error::Result<Option<MergebaseDetails>> {
    let output = String::from_utf8(output).buck_error_context("Failed to parse hg log output")?;
    if output.is_empty() {
        return Ok(None);
    }
    let v: Vec<&str> = output.trim().splitn(3, '\n').collect();
    let mergebase = v
        .first()
        .internal_error("Failed to parse mergebase")?
        .to_string();
    let timestamp = v
        .get(1)
        .and_then(|t| t.parse::<f64>().ok())
        .map(|t| t as u64); // hg returns the fractional seconds
    let global_rev = if let Some(global_rev) = v.get(2) {
        Some(
            global_rev
                .parse::<u64>()
                .buck_error_context("Failed to parse global_rev")?,
        )
    } else {
        None
    };

    Ok(Some(MergebaseDetails {
        mergebase,
        timestamp,
        global_rev,
    }))
}

// Get status between two revisions. If second is None, then it is the working copy.
// Limit the number of results to limit_results. If the number of results is greater than
// limit_results, then return true (and empty vec) to indicate that there are more results.
pub(crate) async fn get_status<D: AsRef<Path>, F: AsRef<str>, S: AsRef<str>>(
    current_dir: D,
    first: F,
    second: Option<S>,
    limit_results: usize,
) -> buck2_error::Result<SaplingGetStatusResult> {
    let mut args = vec!["--traceback", "-mardu", "--rev", first.as_ref()];
    if let Some(ref second) = second {
        args.push("--rev");
        args.push(second.as_ref());
    }

    run_status_command(current_dir, "status", &args, limit_results).await
}

// Get directory differences between two revisions. If second is None, then it is the working copy.
// Limit the number of results to limit_results. If the number of results is greater than
// limit_results return TooManyResults.
pub(crate) async fn get_dir_diff<D: AsRef<Path>, F: AsRef<str>, S: AsRef<str>>(
    current_dir: D,
    first: F,
    second: Option<S>,
    limit_results: usize,
) -> buck2_error::Result<SaplingGetStatusResult> {
    let mut args = vec!["--rev", first.as_ref()];
    if let Some(ref second) = second {
        args.push("--rev");
        args.push(second.as_ref());
    }

    run_status_command(current_dir, "debugdiffdirs", &args, limit_results).await
}

/// Runs a Sapling subcommand that emits status lines, and parses them.
async fn run_status_command<D: AsRef<Path>>(
    current_dir: D,
    subcommand: &'static str,
    args: &[&str],
    limit_results: usize,
) -> buck2_error::Result<SaplingGetStatusResult> {
    let mut output = async_background_command(get_sapling_exe_path())
        .current_dir(current_dir)
        .env("HGPLAIN", "1")
        .arg(subcommand)
        .args(args)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| SaplingError::CommandFailed { subcommand, error })?;

    let stdout = output
        .stdout
        .take()
        .ok_or(SaplingError::MissingStdout { subcommand })?;
    let reader = BufReader::new(stdout);

    let mut status = vec![];
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await? {
        if let Some(status_line) = process_one_status_line(&line)? {
            if status.len() >= limit_results {
                return Ok(SaplingGetStatusResult::TooManyChanges);
            }
            status.push(status_line);
        }
    }

    Ok(SaplingGetStatusResult::Normal(status))
}

//
// Single line looks like:
//    <status> <path>
//
// Where status is one of:
//   M = modified
//   A = added
//   R = removed
//   C = clean
//   ! = missing (deleted by a non-sl command, but still tracked)
//   ? = not tracked
//   I = ignored
//     = origin of the previous file (with --copies)
// Note:
//   Paths can have spaces, but are not quoted.
fn process_one_status_line(line: &str) -> buck2_error::Result<Option<(SaplingStatus, String)>> {
    let mut chars = line.chars();
    // Must include a status and at least one char path.
    if let (Some(status), Some(' '), path) = (chars.next(), chars.next(), chars.collect::<String>())
    {
        let path = path.to_owned();
        Ok(match status {
            'M' => Some((SaplingStatus::Modified, path)),
            'A' => Some((SaplingStatus::Added, path)),
            'R' => Some((SaplingStatus::Removed, path)),
            'C' => Some((SaplingStatus::Clean, path)),
            '!' => Some((SaplingStatus::Missing, path)),
            '?' => Some((SaplingStatus::NotTracked, path)),
            'I' => Some((SaplingStatus::Ignored, path)),
            ' ' => Some((SaplingStatus::Copied, path)),
            _ => None, // Skip all others
        })
    } else {
        Err(SaplingError::InvalidStatusLine {
            line: line.to_owned(),
        }
        .into())
    }
}

#[cfg(test)]
mod tests {
    use buck2_error::Tier;

    use super::*;

    // Stderr from real failures, reduced to the lines the classification depends on.
    const SAPLING_ERROR_EXAMPLES: &[(&str, ErrorTag)] = &[
        (
            r#"Could not start command "hg.real", is it on your PATH? (error: Os { code: 2, kind: NotFound, message: "No such file or directory" })"#,
            ErrorTag::SaplingNotFound,
        ),
        (
            "abort: command failed due to network error (see /tmp/hg-network.log for details)",
            ErrorTag::SaplingNetwork,
        ),
        (
            "error.HttpError: [7] Couldn't connect to server (Failed to connect to mononoke.internal.tfbnw.net port 443 after 8 ms)",
            ErrorTag::SaplingNetwork,
        ),
        (
            "error.TlsError: [35] SSL connect error (OpenSSL/1.1.1zf: error:1408F10B:SSL routines:ssl3_get_record:wrong version number)",
            ErrorTag::Tls,
        ),
        (
            "tls error: [35] SSL connect error (Recv failure: Connection reset by peer)!",
            ErrorTag::Tls,
        ),
        (
            "abort: repository /data/sandcastle/boxes/trunk-hg-full-fbsource not found!",
            ErrorTag::MissingRepo,
        ),
        (
            "abort: '/tmp/scratch' is not inside a repository, but this command requires a repository!",
            ErrorTag::MissingRepo,
        ),
        (
            "abort: cannot initialize working copy: repo /tmp/scratch missing dot dir",
            ErrorTag::MissingRepo,
        ),
        (
            "abort: error getting current working directory: No such file or directory",
            ErrorTag::MissingWorkingDir,
        ),
        (
            "abort: Transport endpoint is not connected: bookmarks.current",
            ErrorTag::IoNotConnected,
        ),
        (
            r#"abort: Permission denied (os error 13) at path "/data/users/build/fbsource""#,
            ErrorTag::IoPermissionDenied,
        ),
        (
            "[BLOCKED] hg command blocked by Crucible VCS isolation: 'hg log' is not an allowed current-workspace query",
            ErrorTag::BlockedByPolicy,
        ),
        // Mentions a repository, but is not a missing one
        (
            "abort: repository requires features unknown to this Mercurial: eden edensparse remotefilelog treestate",
            ErrorTag::Sapling,
        ),
        // Something other than a repository was not found
        ("abort: bookmark 'main' not found!", ErrorTag::Sapling),
        ("abort: internal storage is corrupted", ErrorTag::Sapling),
        ("", ErrorTag::Sapling),
    ];

    #[test]
    fn test_sapling_error_tag() {
        for (stderr, expected) in SAPLING_ERROR_EXAMPLES {
            assert_eq!(&sapling_error_tag(stderr), expected, "stderr: {stderr}");
        }
    }

    /// Keeps the examples above in step with the signatures they document.
    #[test]
    fn test_every_signature_has_an_example() {
        for (index, (signature, _)) in SAPLING_ERROR_SIGNATURES.iter().enumerate() {
            assert!(
                SAPLING_ERROR_EXAMPLES
                    .iter()
                    .any(|(stderr, _)| SAPLING_ERROR_SIGNATURE_SET.matches(stderr).matched(index)),
                "no example matches the signature `{signature}`"
            );
        }
    }

    /// The tag from the stderr has to outrank the `Sapling` tag on the enum, otherwise every
    /// failure keeps being reported as an infra error. Tag rank is positional in
    /// `buck2_error::classify`, so this is easy to break from a distance.
    #[test]
    fn test_categorized_mergebase_error_is_not_infra() {
        let error: buck2_error::Error = SaplingError::Mergebase {
            exit_code: Some(255),
            stderr: "abort: Transport endpoint is not connected".to_owned(),
        }
        .into();

        assert!(error.has_tag(ErrorTag::Sapling));
        assert!(error.has_tag(ErrorTag::IoNotConnected));
        assert_eq!(error.get_tier(), Some(Tier::Environment));
        // The `Sapling` tag is generic, so the root cause alone identifies the error
        assert_eq!(error.category_key(), "IO_NOT_CONNECTED");
    }

    /// Failing to spawn `hg` is the most common Sapling failure, and reports no stderr to
    /// classify, so the tag has to come from the io error instead.
    #[test]
    fn test_missing_sapling_binary_is_environment() {
        let error: buck2_error::Error = SaplingError::CommandFailed {
            subcommand: "log",
            error: io::Error::new(io::ErrorKind::NotFound, "program not found"),
        }
        .into();

        assert!(error.has_tag(ErrorTag::Sapling));
        assert!(error.has_tag(ErrorTag::SaplingNotFound));
        assert_eq!(error.get_tier(), Some(Tier::Environment));
    }

    /// A failure we have no root cause for is tagged `Sapling` and nothing more specific, so it
    /// stays identifiable without claiming a cause we did not establish.
    #[test]
    fn test_uncategorized_errors_have_no_root_cause_tag() {
        for error in [
            SaplingError::CommandFailed {
                subcommand: "log",
                error: io::Error::other("something else"),
            },
            SaplingError::Mergebase {
                exit_code: None,
                stderr: String::new(),
            },
        ] {
            let error: buck2_error::Error = error.into();

            assert!(error.has_tag(ErrorTag::Sapling));
            assert!(!error.has_tag(ErrorTag::SaplingNotFound));
            assert!(
                error.category_key().contains("SAPLING"),
                "{}",
                error.category_key()
            );
        }
    }

    #[test]
    fn test_hg_status_line() -> buck2_error::Result<()> {
        assert_eq!(
            process_one_status_line("M buck2/app/buck2_file_watcher/src/edenfs/sapling.rs")?,
            Some((
                SaplingStatus::Modified,
                "buck2/app/buck2_file_watcher/src/edenfs/sapling.rs".to_owned()
            ))
        );

        assert_eq!(
            process_one_status_line("A buck2/app/buck2_file_watcher/src/edenfs/interface.rs")?,
            Some((
                SaplingStatus::Added,
                "buck2/app/buck2_file_watcher/src/edenfs/interface.rs".to_owned()
            ))
        );

        assert_eq!(
            process_one_status_line("R buck2/app/buck2_file_watcher/src/edenfs/utils.rs")?,
            Some((
                SaplingStatus::Removed,
                "buck2/app/buck2_file_watcher/src/edenfs/utils.rs".to_owned()
            ))
        );

        assert_eq!(
            process_one_status_line("! buck2/app/buck2_file_watcher/src/edenfs/sapling.rs")?,
            Some((
                SaplingStatus::Missing,
                "buck2/app/buck2_file_watcher/src/edenfs/sapling.rs".to_owned()
            ))
        );

        assert_eq!(
            process_one_status_line("? buck2/app/buck2_file_watcher/src/edenfs/sapling.rs")?,
            Some((
                SaplingStatus::NotTracked,
                "buck2/app/buck2_file_watcher/src/edenfs/sapling.rs".to_owned()
            ))
        );

        // Space in path
        assert_eq!(
            process_one_status_line("M ovrsource-legacy/unity/socialvr/modules/wb_unity_asset_bundles/Assets/MetaHorizonUnityAssetBundle/Editor/Unity Dependencies/ABDataSource.cs")?,
            Some((
                SaplingStatus::Modified,
                "ovrsource-legacy/unity/socialvr/modules/wb_unity_asset_bundles/Assets/MetaHorizonUnityAssetBundle/Editor/Unity Dependencies/ABDataSource.cs".to_owned()
            ))
        );

        assert_eq!(
            process_one_status_line("C buck2/app/buck2_file_watcher/src/edenfs/sapling.rs")?,
            Some((
                SaplingStatus::Clean,
                "buck2/app/buck2_file_watcher/src/edenfs/sapling.rs".to_owned()
            ))
        );

        assert_eq!(
            process_one_status_line("I buck2/app/buck2_file_watcher/src/edenfs/sapling.rs")?,
            Some((
                SaplingStatus::Ignored,
                "buck2/app/buck2_file_watcher/src/edenfs/sapling.rs".to_owned()
            ))
        );

        assert_eq!(
            process_one_status_line("  buck2/app/buck2_file_watcher/src/edenfs/sapling.rs")?,
            Some((
                SaplingStatus::Copied,
                "buck2/app/buck2_file_watcher/src/edenfs/sapling.rs".to_owned()
            ))
        );

        assert!(process_one_status_line("NO").is_err());

        // Invalid status (missing status), but valid path with space in it
        assert!(
            process_one_status_line(" ovrsource-legacy/unity/socialvr/modules/wb_unity_asset_bundles/Assets/MetaHorizonUnityAssetBundle/Editor/Unity Dependencies/ABDataSource.cs")
                .is_err());

        // Malformed status (no space)
        assert!(
            process_one_status_line("Mbuck2/app/buck2_file_watcher/src/edenfs/sapling.rs").is_err()
        );

        // Malformed status (colon instead of space)
        assert!(
            process_one_status_line("M:buck2/app/buck2_file_watcher/src/edenfs/sapling.rs")
                .is_err()
        );

        Ok(())
    }

    #[test]
    fn test_parse_log_output() -> buck2_error::Result<()> {
        // the format is {node}\n{date}\n{global_rev}
        let output =
            "71de423b796418e8ff5300dbe9bd9ad3aef63a9c\n1739790802.028800\n1020164040".to_owned();
        let details = parse_log_output(output.as_bytes().to_vec())?.unwrap();
        assert_eq!(
            details.mergebase,
            "71de423b796418e8ff5300dbe9bd9ad3aef63a9c"
        );
        assert_eq!(details.timestamp, Some(1739790802));
        assert_eq!(details.global_rev, Some(1020164040));
        Ok(())
    }

    #[test]
    fn test_parse_log_output_no_global_rev() -> buck2_error::Result<()> {
        // Not all repos have global revision
        let output = "71de423b796418e8ff5300dbe9bd9ad3aef63a9c\n1739790802.028800\n".to_owned();
        let details = parse_log_output(output.as_bytes().to_vec())?.unwrap();
        assert_eq!(
            details.mergebase,
            "71de423b796418e8ff5300dbe9bd9ad3aef63a9c"
        );
        assert_eq!(details.global_rev, None);
        assert_eq!(details.timestamp, Some(1739790802));
        Ok(())
    }
}
