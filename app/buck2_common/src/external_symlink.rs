/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use allocative::Allocative;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use dupe::Dupe;
use pagable::Pagable;

type Utf8Path = String;

/// Represents a path containing a symlink that resolves to an external path.
/// What path does the symlink resolve to (`abs_target`), and what goes after
/// that (`remaining_path`).
///
/// E.g. foo/bar/file, where foo/bar -> /root, would be represented as:
///      ExternalSymlink { abs_target: "/root", remaining_path: "file" }
#[derive(Debug, Hash, PartialEq, Eq, Clone, Allocative, Pagable)]
pub struct ExternalSymlink {
    /// The external target the symlink resolves to.
    // We can't use AbsPathBuf because there might be "." or ".." in the path
    abs_target: Utf8Path,
    /// What goes after the external target path.
    remaining_path: ForwardRelativePathBuf,
}

// `Hash` hashes both paths' string content, so it is already strong.
strong_hash::impl_strong_hash_for_impl_hash!(ExternalSymlink);

impl fmt::Display for ExternalSymlink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_path_buf().display())
    }
}

/// Returns the target string with separators normalized for POSIX consumers.
///
/// `ExternalSymlink` targets may be serialized verbatim into `RE::SymlinkNode.target`
/// and materialized as symlinks on Linux RE workers, which cannot resolve `\` as a
/// path separator. On Windows hosts `read_link` returns paths with `\` separators
/// that must be normalized before they reach the wire.
///
/// `host_is_windows` is taken as a parameter (rather than read from `cfg!`) so the
/// invariant can be exercised from both branches in unit tests on any platform.
fn normalize_target_for_re(s: String, host_is_windows: bool) -> String {
    if host_is_windows {
        s.replace('\\', "/")
    } else {
        s
    }
}

impl ExternalSymlink {
    pub fn new(
        abs_target: PathBuf,
        remaining_path: ForwardRelativePathBuf,
    ) -> buck2_error::Result<Self> {
        let abs_target = match abs_target.into_os_string().into_string() {
            Ok(string) => normalize_target_for_re(string, cfg!(windows)),
            Err(os_string) => {
                return Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Tier0,
                    "Found external symlink that's not utf-8. Lossy representation: {}",
                    os_string.to_string_lossy()
                ));
            }
        };
        Ok(Self {
            abs_target,
            remaining_path,
        })
    }

    pub fn target(&self) -> &Path {
        Path::new(&self.abs_target)
    }

    pub fn remaining_path(&self) -> &ForwardRelativePath {
        &self.remaining_path
    }

    /// Returns the complete path as a [`PathBuf`]
    pub fn to_path_buf(&self) -> PathBuf {
        if !self.remaining_path.is_empty() {
            Path::new(&self.abs_target).join(self.remaining_path.as_str())
        } else {
            Path::new(&self.abs_target).to_owned()
        }
    }

    /// Returns a new `ExternalSymlink` with its target being the full target
    /// of `self` (i.e. `{self.target}/{self.remaining_path}`).
    pub fn with_full_target(self: &Arc<Self>) -> buck2_error::Result<Arc<Self>> {
        if !self.remaining_path.is_empty() {
            Ok(Arc::new(Self::new(
                self.to_path_buf(),
                ForwardRelativePathBuf::default(),
            )?))
        } else {
            Ok(self.dupe())
        }
    }

    /// Returns a new `ExternalSymlink` with `remaining_path` discarded.
    pub fn without_remaining_path(self: &Arc<Self>) -> Arc<Self> {
        if !self.remaining_path.is_empty() {
            Arc::new(Self {
                abs_target: self.abs_target.clone(),
                remaining_path: ForwardRelativePathBuf::default(),
            })
        } else {
            self.dupe()
        }
    }

    pub fn target_str(&self) -> &str {
        &self.abs_target
    }

    /// Given a `path = "[...a]/[...b]"`, and `remaining_path = Some("[...b]")`,
    /// returns `Some("[...a]")`. It returns `None` if `path` doesn't end with
    /// `remaining_path`.
    pub fn fix_source_path<'a>(
        &self,
        path: &'a ForwardRelativePath,
    ) -> Option<&'a ForwardRelativePath> {
        path.strip_suffix_opt(&self.remaining_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_target_for_re_replaces_backslashes_when_host_is_windows() {
        let out = normalize_target_for_re(r"\mnt\gvfs\third-party2\openssl\lib".to_owned(), true);
        assert_eq!(out, "/mnt/gvfs/third-party2/openssl/lib");
        assert!(
            !out.contains('\\'),
            "normalize_target_for_re should replace all backslashes with forward slashes on Windows, got: {out}",
        );
    }

    #[test]
    fn normalize_target_for_re_preserves_backslashes_when_host_is_not_windows() {
        // On Linux, `\` is a legal filename character and must not be munged.
        let input = r"foo\with\literal\backslashes".to_owned();
        let out = normalize_target_for_re(input.clone(), false);
        assert_eq!(out, input);
    }

    #[test]
    fn normalize_target_for_re_is_idempotent_on_already_posix_paths() {
        let input = "/mnt/gvfs/third-party2/openssl/lib".to_owned();
        assert_eq!(
            normalize_target_for_re(input.clone(), true),
            normalize_target_for_re(input.clone(), false),
        );
        assert_eq!(normalize_target_for_re(input.clone(), true), input);
    }

    /// End-to-end regression for the Windows→Linux RE symlink-target bug:
    /// `ExternalSymlink` targets must be POSIX-style strings because they are
    /// serialized verbatim into `RE::SymlinkNode.target` and materialized as
    /// Linux symlinks on RE workers. Backslashes from a Windows `read_link`
    /// result would otherwise produce ENOENT on dereference. This test is
    /// Windows-gated because the constructor reads `cfg!(windows)` directly;
    /// the helper is exercised on all platforms via the tests above.
    #[cfg(windows)]
    #[test]
    fn new_normalizes_windows_path_separators_on_windows_hosts() {
        let sym = ExternalSymlink::new(
            PathBuf::from(r"\mnt\gvfs\third-party2\openssl\lib"),
            ForwardRelativePathBuf::default(),
        )
        .unwrap();
        assert_eq!(sym.target_str(), "/mnt/gvfs/third-party2/openssl/lib");
        assert!(
            !sym.target_str().contains('\\'),
            "ExternalSymlink::new must normalize backslashes to forward slashes on Windows, got: {}",
            sym.target_str(),
        );
    }
}
