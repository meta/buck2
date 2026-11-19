/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::Hash;
use std::sync::Arc;

use allocative::Allocative;
use buck2_core::buck2_env;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::fs::i_project_rel_path::IProjectRelativePath;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::RelativePath;
use buck2_fs::paths::RelativePathBuf;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_util::arc_str::PackedStringSlice;
use derive_more::Display;
use dupe::Dupe;
use pagable::Pagable;
use variant_name::VariantName;

use crate::cas_digest::CasDigest;
use crate::cas_digest::CasDigestConfig;
use crate::cas_digest::CasDigestKind;
use crate::external_symlink::ExternalSymlink;

/// std::fs::FileType is an opaque type that isn't constructible. This is
/// basically the equivalent.
#[derive(
    Copy, Clone, Dupe, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Allocative, Pagable
)]
pub enum FileType {
    Directory,
    File,
    Symlink,
    /// Some special files (for example, unix domain sockets) will have a
    /// filetype that we don't recognize.
    Unknown,
}

impl FileType {
    /// A noun for error messages.
    pub fn describe(self) -> &'static str {
        match self {
            FileType::Directory => "directory",
            FileType::File => "file",
            FileType::Symlink => "symlink",
            FileType::Unknown => "special file",
        }
    }
}

impl From<std::fs::FileType> for FileType {
    fn from(fs_type: std::fs::FileType) -> Self {
        if fs_type.is_file() {
            FileType::File
        } else if fs_type.is_dir() {
            FileType::Directory
        } else if fs_type.is_symlink() {
            FileType::Symlink
        } else {
            FileType::Unknown
        }
    }
}

impl FileType {
    pub fn is_dir(&self) -> bool {
        matches!(self, FileType::Directory)
    }

    pub fn is_file(&self) -> bool {
        matches!(self, FileType::File)
    }

    pub fn is_symlink(&self) -> bool {
        matches!(self, FileType::Symlink)
    }
}

/// Transient IO-boundary type returned from `IoProvider::read_dir_impl`. Not
/// stored in DICE — entries are immediately sorted and packed into the more
/// compact [`ReadDirOutput`] representation.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Allocative)]
pub struct RawDirEntry {
    // Put the `file_name` first so we sort by it (which is what people expect)
    pub file_name: FileNameBuf,
    pub file_type: FileType,
}

#[derive(Clone, Eq, PartialEq, Hash, Debug, Dupe, Allocative, Pagable)]
pub struct ReadDirOutput {
    /// Sorted by file name.
    pub included: PackedStringSlice<FileName, FileType>,
}

impl ReadDirOutput {
    /// Is the file name in the directory listing. Ignores files that were explicitly ignored.
    pub fn contains(&self, file_name: &FileName) -> bool {
        self.included.contains(file_name)
    }
}

#[derive(Allocative, Debug, Pagable)]
pub struct FileDigestKind {
    _private: (),
}

impl CasDigestKind for FileDigestKind {}

pub type FileDigest = CasDigest<FileDigestKind>;

pub use crate::cas_digest::TrackedFileDigest;

#[derive(Copy, Dupe, Clone)]
pub struct FileDigestConfig {
    config: CasDigestConfig,
}

impl FileDigestConfig {
    /// Obtain a FileDigestConfig for hashing source files.
    pub fn source(config: CasDigestConfig) -> Self {
        Self {
            config: config.source_files_config(),
        }
    }

    /// Obtain a FileDigestConfig for hashing output files.
    pub fn build(config: CasDigestConfig) -> Self {
        Self { config }
    }

    /// Obtain the underlying CasDigestConfig.
    pub fn as_cas_digest_config(&self) -> CasDigestConfig {
        self.config
    }
}

impl FileDigest {
    /// Obtain the digest of the file if you can.
    pub fn from_file(file: &AbsPath, config: FileDigestConfig) -> buck2_error::Result<Self> {
        if !buck2_env!("BUCK2_DISABLE_FILE_ATTR", bool)? {
            if let Some(digest) = Self::from_file_attr(file, config) {
                return Ok(digest);
            }
        }

        Self::from_file_disk(file, config)
    }

    /// Read the file from the xattr, or skip if it's not available.
    #[cfg(unix)]
    fn from_file_attr(file: &AbsPath, config: FileDigestConfig) -> Option<Self> {
        use crate::cas_digest::RawDigest;

        enum Digest {
            Sha1,
            Sha256,
            Blake3Keyed,
        }

        let digest = if config.as_cas_digest_config().allows_sha1() {
            Digest::Sha1
        } else if config.as_cas_digest_config().allows_sha256() {
            Digest::Sha256
        } else if config.as_cas_digest_config().allows_blake3_keyed() {
            Digest::Blake3Keyed
        } else {
            return None;
        };

        let meta = fs_util::symlink_metadata(file).ok()?;

        // This really shouldn't happen (and it better not because we're about to read the xattr of
        // a symlink otherwise) but this method doesn't have the ability to return an error.
        if meta.is_symlink() {
            return None;
        }

        let raw_digest = match digest {
            Digest::Sha1 => xattr::get(file.as_maybe_relativized(), "user.sha1")
                .ok()
                .flatten()
                .and_then(|v| RawDigest::parse_sha1(&v).ok()),
            Digest::Sha256 => xattr::get(file.as_maybe_relativized(), "user.sha256")
                .ok()
                .flatten()
                .and_then(|v| RawDigest::parse_sha256(&v).ok()),
            // NOTE: Eden returns *keyed* blake3 in user.blake3, so we use that.
            Digest::Blake3Keyed => xattr::get(file.as_maybe_relativized(), "user.blake3")
                .ok()
                .flatten()
                .and_then(|v| RawDigest::parse_blake3_keyed(&v).ok()),
        };

        raw_digest.map(|raw| Self::new(raw, meta.len()))
    }

    /// Windows doesn't support extended file attributes.
    #[cfg(windows)]
    fn from_file_attr(_file: &AbsPath, _config: FileDigestConfig) -> Option<Self> {
        None
    }

    /// Get the digest from disk. You should usually prefer `from_file`
    /// which also uses faster methods of getting the SHA1 if it can.
    fn from_file_disk(file: &AbsPath, config: FileDigestConfig) -> buck2_error::Result<Self> {
        let f = fs_util::open_file(file).categorize_internal()?;
        FileDigest::from_reader(f, config.as_cas_digest_config())
    }
}

/// Stores the relevant metadata for a file.
// New fields should be added as needed, and unused fields removed.
#[derive(
    Debug,
    Dupe,
    Hash,
    PartialEq,
    Eq,
    Clone,
    Display,
    Allocative,
    Pagable,
    strong_hash::StrongHash
)]
#[display("File(digest={}, is_executable={})", digest, is_executable)]
pub struct FileMetadata {
    pub digest: TrackedFileDigest,
    pub is_executable: bool,
}

impl FileMetadata {
    /// Metadata of an empty file
    pub fn empty(config: CasDigestConfig) -> Self {
        Self {
            digest: TrackedFileDigest::empty(config),
            is_executable: false,
        }
    }

    pub fn with_executable(mut self, executable: bool) -> Self {
        self.is_executable = executable;
        self
    }
}

/// Stores the relevant metadata for a path.
#[derive(Debug, Dupe, PartialEq, Eq, Clone)]
pub enum PathMetadata {
    File(FileMetadata),
    ExternalSymlink(Arc<ExternalSymlink>),
    Directory,
}

#[derive(Debug, PartialEq, Dupe, Eq, Clone, Allocative, VariantName, Pagable)]
pub enum RawPathMetadata<T = CellPath> {
    Symlink { at: T, to: RawSymlink<T> },
    File(FileMetadata),
    Directory,
}

/// Represents a relative symlink, and stores the symlink's target path.
#[derive(Debug, Display, Hash, Eq, PartialEq, Clone, Allocative, Pagable)]
pub struct Symlink(RelativePathBuf);

// `Hash` hashes the target string's content, so it is already strong.
strong_hash::impl_strong_hash_for_impl_hash!(Symlink);

impl Symlink {
    pub fn new(target: RelativePathBuf) -> Self {
        Self(target)
    }

    /// Returns the path the symlink points to.
    pub fn target(&self) -> &RelativePath {
        self.0.as_relative_path()
    }

    /// Creates a new `Symlink` from `self`, such that its target is
    /// `src_relative_to_dest/self.target()`.
    ///
    /// This solves a specific problem: symlink at path `src` points to an
    /// artifact at path `src/target`. We move the symlink to `dest`, but we
    /// want to keep linking to the same artifact. How can we adjust the target
    /// in order to achieve that? We need to know how to get to `src` from
    /// `dest`, which is what `src_relative_to_dest` tells us.
    pub fn relativized<P: AsRef<RelativePath>>(&self, src_relative_to_dest: P) -> Self {
        // FIXME(rafaelc): we don't need to normalize the target anymore!
        let relativized_t = src_relative_to_dest.as_ref().join_normalized(&self.0);
        Self(relativized_t)
    }
}

#[derive(
    Debug,
    Dupe,
    Hash,
    PartialEq,
    Eq,
    Clone,
    Allocative,
    VariantName,
    Pagable
)]
pub enum RawSymlink<T> {
    Relative(T, Arc<Symlink>),
    External(Arc<ExternalSymlink>),
}

impl<T> RawSymlink<T> {
    pub fn map<O>(self, f: impl Fn(T) -> O) -> RawSymlink<O> {
        match self {
            RawSymlink::Relative(target, raw) => RawSymlink::Relative(f(target), raw),
            RawSymlink::External(external) => RawSymlink::External(external),
        }
    }
}

impl RawSymlink<IProjectRelativePath> {
    /// The target of this symlink, continued by `rest`: what a path that goes through the symlink
    /// and then follows `rest` points at.
    pub fn continued_by(self, rest: &ForwardRelativePath) -> buck2_error::Result<Self> {
        Ok(match self {
            RawSymlink::Relative(target, raw) => RawSymlink::Relative(
                target.join(rest),
                Arc::new(Symlink::new(raw.target().join(rest.as_str()))),
            ),
            RawSymlink::External(external) => RawSymlink::External(Arc::new(ExternalSymlink::new(
                external.target().to_owned(),
                external.remaining_path().join(rest),
            )?)),
        })
    }
}

/// Stores the relevant metadata for a path.
#[derive(Debug, Dupe, PartialEq, Eq, Clone)]
pub enum PathMetadataOrRedirection<T = CellPath> {
    PathMetadata(PathMetadata),
    Redirection(T, Arc<Symlink>),
}

impl<T> PathMetadataOrRedirection<T> {
    pub fn map<O>(self, f: impl Fn(T) -> O) -> PathMetadataOrRedirection<O> {
        match self {
            Self::PathMetadata(meta) => PathMetadataOrRedirection::PathMetadata(meta),
            Self::Redirection(r, r2) => PathMetadataOrRedirection::Redirection(f(r), r2),
        }
    }
}

impl<T> From<RawPathMetadata<T>> for PathMetadataOrRedirection<T> {
    fn from(meta: RawPathMetadata<T>) -> Self {
        match meta {
            RawPathMetadata::Directory => {
                PathMetadataOrRedirection::PathMetadata(PathMetadata::Directory)
            }
            RawPathMetadata::File(file) => {
                PathMetadataOrRedirection::PathMetadata(PathMetadata::File(file))
            }
            RawPathMetadata::Symlink {
                at: _,
                to: RawSymlink::Relative(a, b),
            } => PathMetadataOrRedirection::Redirection(a, b),
            RawPathMetadata::Symlink {
                at: _,
                to: RawSymlink::External(e),
            } => PathMetadataOrRedirection::PathMetadata(PathMetadata::ExternalSymlink(e)),
        }
    }
}

impl<T> RawPathMetadata<T> {
    pub fn map<O>(self, f: impl Fn(T) -> O) -> RawPathMetadata<O> {
        match Self::try_map::<O, RawPathMetadata<O>>(self, |v| Ok(f(v))) {
            Ok(out) => out,
            Err(e) => e,
        }
    }

    pub fn try_map<O, E>(self, f: impl Fn(T) -> Result<O, E>) -> Result<RawPathMetadata<O>, E> {
        match self {
            Self::Directory => Ok(RawPathMetadata::Directory),
            Self::File(file) => Ok(RawPathMetadata::File(file)),
            Self::Symlink {
                at,
                to: RawSymlink::Relative(dest, dest_rel),
            } => Ok(RawPathMetadata::Symlink {
                at: f(at)?,
                to: RawSymlink::Relative(f(dest)?, dest_rel),
            }),
            Self::Symlink {
                at,
                to: RawSymlink::External(e),
            } => Ok(RawPathMetadata::Symlink {
                at: f(at)?,
                to: RawSymlink::External(e),
            }),
        }
    }
}

impl<T> From<PathMetadata> for PathMetadataOrRedirection<T> {
    fn from(meta: PathMetadata) -> Self {
        Self::PathMetadata(meta)
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Borrow;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hash;
    use std::hash::Hasher;

    use super::*;
    use crate::cas_digest::CasDigestConfig;
    use crate::file_ops::metadata::FileDigest;
    use crate::file_ops::metadata::TrackedFileDigest;

    #[test]
    fn test_tracked_file_digest_equivalence() {
        let digest_config = CasDigestConfig::testing_default();
        let digest = FileDigest::from_content(b"foo", digest_config);
        let tracked_digest = TrackedFileDigest::new(digest.dupe(), digest_config);

        assert_eq!(&digest, tracked_digest.borrow());

        let mut hasher_digest = DefaultHasher::new();
        digest.hash(&mut hasher_digest);

        let mut hasher_tracked_digest = DefaultHasher::new();
        tracked_digest.hash(&mut hasher_tracked_digest);

        assert_eq!(hasher_digest.finish(), hasher_tracked_digest.finish());
        assert_eq!(digest.to_string(), tracked_digest.to_string());
    }
}
