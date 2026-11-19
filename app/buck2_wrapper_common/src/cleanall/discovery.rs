/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ffi::OsStr;
use std::ffi::OsString;
#[cfg(test)]
use std::io;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use buck2_hash::BuckMutSet;
use tokio::fs;

use crate::BUCKD_LIFECYCLE;
use crate::DEFAULT_ISOLATION_DIR;

#[cfg(test)]
const TEST_READ_DIR_ERROR_PATH: &str = "__buck2_test_read_dir_error__";

#[derive(Clone, Copy)]
enum BuckdPathLayout {
    Unix,
    Windows,
}

impl BuckdPathLayout {
    fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

#[derive(Debug, Eq, Hash, PartialEq)]
pub(super) struct CleanallTarget {
    pub(super) project_root: PathBuf,
    pub(super) isolation_dir: String,
}

async fn find_lifecycle_markers(buckd_root: &Path) -> Vec<PathBuf> {
    let mut directories = vec![buckd_root.to_owned()];
    let mut markers = Vec::new();
    let lifecycle_filename = OsStr::new(BUCKD_LIFECYCLE);

    while let Some(directory) = directories.pop() {
        #[cfg(test)]
        let read_result = if directory.ends_with(TEST_READ_DIR_ERROR_PATH) {
            Err(io::Error::other("injected read_dir error"))
        } else {
            fs::read_dir(&directory).await
        };
        #[cfg(not(test))]
        let read_result = fs::read_dir(&directory).await;

        let mut entries = match read_result {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        // Skip traversing a directory on the first error
        // Keep invoking `next_entry` could cause us to hit some
        // persistent errors and hang indefinitely.
        // Note that isolation directory names are arbitrary and
        // nested checkouts are valid, so discovery must walk
        // buckd root down to all of its leaf dirs to find all valid
        // daemon dirs.
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if file_type.is_file() && entry.file_name() == lifecycle_filename {
                markers.push(path);
                continue;
            }

            // buckd root does not use symlinks, so only follow real directories.
            if file_type.is_dir() {
                directories.push(path);
            }
        }
    }

    markers
}

fn decode_lifecycle_marker(
    buckd_root: &Path,
    marker: &Path,
    layout: BuckdPathLayout,
) -> Vec<CleanallTarget> {
    if marker.file_name() != Some(OsStr::new(BUCKD_LIFECYCLE)) {
        return Vec::new();
    }

    let Some(daemon_dir) = marker
        .parent()
        .and_then(|parent| parent.strip_prefix(buckd_root).ok())
    else {
        return Vec::new();
    };
    let Some(components) = daemon_dir
        .components()
        .map(|component| match component {
            Component::Normal(component) => Some(component.to_owned()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
    else {
        return Vec::new();
    };
    let Some((isolation_dir, project_components)) = components.split_last() else {
        return Vec::new();
    };
    let Some(isolation_dir) = isolation_dir.to_str() else {
        return Vec::new();
    };
    if project_components.is_empty() {
        return Vec::new();
    }

    let project_roots = match layout {
        BuckdPathLayout::Unix => {
            let mut project_root = PathBuf::from("/");
            project_root.extend(project_components);
            vec![project_root]
        }
        BuckdPathLayout::Windows => decode_windows_project_roots(project_components),
    };

    project_roots
        .into_iter()
        .map(|project_root| CleanallTarget {
            project_root,
            isolation_dir: isolation_dir.to_owned(),
        })
        .collect()
}

/// Windows daemon paths encode drive and UNC prefixes as ordinary directory
/// components, meaning a leading letter can be either a drive or a UNC server.
/// Return every drive and UNC reconstruction so callers can validate candidates.
/// Device namespace paths are not handled.
fn decode_windows_project_roots(components: &[OsString]) -> Vec<PathBuf> {
    let Some((prefix, remaining)) = components.split_first() else {
        return Vec::new();
    };

    let drive_root = prefix
        .to_str()
        // Check for valid windows drives
        .is_some_and(|prefix| prefix.len() == 1 && prefix.as_bytes()[0].is_ascii_alphabetic())
        .then(|| {
            let mut project_root = OsString::new();
            project_root.push(r"\\?\");
            project_root.push(prefix);
            project_root.push(":");
            if remaining.is_empty() {
                project_root.push("\\");
            } else {
                for component in remaining {
                    project_root.push("\\");
                    project_root.push(component);
                }
            }
            PathBuf::from(project_root)
        });

    let unc_root = remaining.split_first().map(|(share, remaining)| {
        let mut project_root = OsString::from("\\\\");
        project_root.push(prefix);
        project_root.push("\\");
        project_root.push(share);
        for component in remaining {
            project_root.push("\\");
            project_root.push(component);
        }
        PathBuf::from(project_root)
    });

    drive_root.into_iter().chain(unc_root).collect()
}

fn cleanall_target_priority(target: &CleanallTarget) -> u8 {
    let is_default_isolation_dir = target.isolation_dir == DEFAULT_ISOLATION_DIR;

    if cfg!(fbcode_build) {
        let is_fbsource_checkout = target
            .project_root
            .file_name()
            .and_then(OsStr::to_str)
            // Check for all checkouts containing name fbsource so we can also
            // catch other common names like `fbsource2` and `fbsource3`.
            .is_some_and(|name| name.contains("fbsource"));

        match (is_fbsource_checkout, is_default_isolation_dir) {
            (true, true) => 0,
            (false, true) => 1,
            (true, false) => 2,
            (false, false) => 3,
        }
    } else if is_default_isolation_dir {
        0
    } else {
        1
    }
}

fn targets_from_markers(
    buckd_root: &Path,
    markers: impl IntoIterator<Item = PathBuf>,
    layout: BuckdPathLayout,
) -> Vec<CleanallTarget> {
    let mut targets: Vec<_> = markers
        .into_iter()
        .flat_map(|marker| decode_lifecycle_marker(buckd_root, &marker, layout))
        .collect::<BuckMutSet<_>>()
        .into_iter()
        .collect();
    targets.sort_unstable_by(|left, right| {
        cleanall_target_priority(left)
            .cmp(&cleanall_target_priority(right))
            .then_with(|| left.project_root.cmp(&right.project_root))
            .then_with(|| left.isolation_dir.cmp(&right.isolation_dir))
    });
    targets
}

/// Registry entries can outlive their project roots, so callers must validate
/// each discovered target before invoking cleanup.
pub(super) async fn discover_cleanall_targets(buckd_root: &Path) -> Vec<CleanallTarget> {
    // TODO(scottcao): Also run separate buck-out discovery to discover all possible cleanall targets
    let markers = find_lifecycle_markers(buckd_root).await;
    targets_from_markers(buckd_root, markers, BuckdPathLayout::current())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    async fn create_lifecycle_marker(
        buckd_root: &Path,
        project_root: &Path,
        isolation_dir: &str,
    ) -> PathBuf {
        let marker = buckd_root
            .join(
                project_root
                    .strip_prefix("/")
                    .expect("temporary project should be absolute"),
            )
            .join(isolation_dir)
            .join(BUCKD_LIFECYCLE);
        fs::create_dir_all(marker.parent().expect("marker should have a parent"))
            .await
            .expect("daemon directory should be created");
        fs::write(&marker, [])
            .await
            .expect("lifecycle marker should be created");
        marker
    }

    #[test]
    fn decodes_unix_lifecycle_path() {
        let buckd_root = Path::new("/home/user/.buck/buckd");
        let marker = buckd_root.join("data/users/user/repo/custom/buckd.lifecycle");

        assert_eq!(
            decode_lifecycle_marker(buckd_root, &marker, BuckdPathLayout::Unix),
            vec![CleanallTarget {
                project_root: PathBuf::from("/data/users/user/repo"),
                isolation_dir: String::from("custom"),
            }]
        );
    }

    #[cfg(unix)]
    #[test]
    fn ignores_non_utf8_isolation_dir() {
        use std::os::unix::ffi::OsStringExt;

        let buckd_root = Path::new("/home/user/.buck/buckd");
        let marker = buckd_root
            .join("data/users/user/repo")
            .join(OsString::from_vec(vec![0xff]))
            .join(BUCKD_LIFECYCLE);

        assert!(decode_lifecycle_marker(buckd_root, &marker, BuckdPathLayout::Unix).is_empty());
    }

    #[test]
    fn decodes_windows_lifecycle_paths() {
        let buckd_root = Path::new("registry");

        assert_eq!(
            decode_lifecycle_marker(
                buckd_root,
                &buckd_root.join("C/repo/nested/v2/buckd.lifecycle"),
                BuckdPathLayout::Windows,
            ),
            vec![
                CleanallTarget {
                    project_root: PathBuf::from(r"\\?\C:\repo\nested"),
                    isolation_dir: String::from("v2"),
                },
                CleanallTarget {
                    project_root: PathBuf::from(r"\\C\repo\nested"),
                    isolation_dir: String::from("v2"),
                },
            ]
        );
        assert_eq!(
            decode_lifecycle_marker(
                buckd_root,
                &buckd_root.join("server/share/repo/v2/buckd.lifecycle"),
                BuckdPathLayout::Windows,
            ),
            vec![CleanallTarget {
                project_root: PathBuf::from(r"\\server\share\repo"),
                isolation_dir: String::from("v2"),
            }]
        );
    }

    #[test]
    fn orders_and_deduplicates_cleanall_targets() {
        let buckd_root = Path::new("/registry");
        let marker = |path: &str| buckd_root.join(path).join(BUCKD_LIFECYCLE);
        let target = |project_root: &str, isolation_dir: &str| CleanallTarget {
            project_root: PathBuf::from(project_root),
            isolation_dir: isolation_dir.to_owned(),
        };

        let targets = targets_from_markers(
            buckd_root,
            [
                marker("d/repo/custom"),
                marker("c/fbsource_backup/beta"),
                marker("b/checkout/v2"),
                marker("z/fbsource/v2"),
                marker("c/fbsource_backup/alpha"),
                marker("a/my_fbsource_checkout/v2"),
                marker("z/fbsource/v2"),
            ],
            BuckdPathLayout::Unix,
        );

        let expected = if cfg!(fbcode_build) {
            vec![
                target("/a/my_fbsource_checkout", "v2"),
                target("/z/fbsource", "v2"),
                target("/b/checkout", "v2"),
                target("/c/fbsource_backup", "alpha"),
                target("/c/fbsource_backup", "beta"),
                target("/d/repo", "custom"),
            ]
        } else {
            vec![
                target("/a/my_fbsource_checkout", "v2"),
                target("/b/checkout", "v2"),
                target("/z/fbsource", "v2"),
                target("/c/fbsource_backup", "alpha"),
                target("/c/fbsource_backup", "beta"),
                target("/d/repo", "custom"),
            ]
        };

        assert_eq!(targets, expected);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn discovers_lifecycle_marker_without_buck_out_or_live_daemon() {
        let temp = tempfile::tempdir().expect("temporary directory should be created");
        let buckd_root = temp.path().join("registry");
        let project_root = temp.path().join("project");
        let isolation_dir = "v2";
        let marker = create_lifecycle_marker(&buckd_root, &project_root, isolation_dir).await;
        let targets = discover_cleanall_targets(&buckd_root).await;

        assert_eq!(
            targets,
            vec![CleanallTarget {
                project_root,
                isolation_dir: String::from(isolation_dir),
            }]
        );
        assert!(
            !fs::try_exists(
                marker
                    .parent()
                    .expect("marker should have a parent")
                    .join("buckd.info")
            )
            .await
            .expect("daemon directory should be readable"),
            "discovery should not require a live daemon"
        );
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn discovery_continues_after_traversal_error() {
        let temp = tempfile::tempdir().expect("temporary directory should be created");
        let buckd_root = temp.path().join("registry");
        let project_root = temp.path().join("project");
        let isolation_dir = "v2";
        create_lifecycle_marker(&buckd_root, &project_root, isolation_dir).await;
        let failed_marker = buckd_root
            .join(TEST_READ_DIR_ERROR_PATH)
            .join("project")
            .join(isolation_dir)
            .join(BUCKD_LIFECYCLE);
        fs::create_dir_all(
            failed_marker
                .parent()
                .expect("failed marker should have a parent"),
        )
        .await
        .expect("failed marker directory should be created");
        fs::write(&failed_marker, [])
            .await
            .expect("failed marker should be created");
        let targets = discover_cleanall_targets(&buckd_root).await;

        assert_eq!(
            targets,
            vec![CleanallTarget {
                project_root,
                isolation_dir: String::from(isolation_dir),
            }]
        );
    }
}
