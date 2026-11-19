/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use compact_str::CompactString;
use pagable::Pagable;

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum ContentBasedPathHashError {
    #[error("Content hash must be 16 hex digits, got: `{0}`")]
    NotLongEnough(String),
}

/// Hash of some content, serialized as a hex string.
///
/// The `Specified` variant uses `CompactString`, which stores the 16 ASCII hex
/// characters inline and provides constant-time string access without a heap
/// allocation per hash.
#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    Hash,
    Allocative,
    derive_more::Display,
    Ord,
    PartialOrd,
    Pagable,
    strong_hash::StrongHash
)]
pub enum ContentBasedPathHash {
    Specified(#[pagable(flatten_serde)] CompactString),
    OutputArtifact,
    /// When running aquery we don't have content hashes for any of our inputs, so we
    /// just use a placeholder value instead.
    AqueryPlaceholder,
    /// If we're not using the configuration hash in the scratch path, then just use a
    /// placeholder value.
    Scratch,
    RelativePathResolution,
    /// When comparing actions using dep-files, if an unused input changes, it's
    /// content-based path will also change, and that can affect (a) the command-line
    /// digest, and (b) the content-based path of used-inputs (e.g. if they are part
    /// of the same directory). To avoid this affecting the dep-files, we use a
    /// non-content-based representation for dep-file comparisons.
    DepFilesPlaceholder,
}

impl ContentBasedPathHash {
    pub fn new(bytes: &[u8]) -> buck2_error::Result<ContentBasedPathHash> {
        if bytes.len() < 8 {
            return Err(ContentBasedPathHashError::NotLongEnough(hex::encode(bytes)).into());
        }

        // 8 input bytes encode to exactly 16 hex characters, which matches the
        // length of `value`, so `encode_to_slice` cannot fail.
        let mut value = [0u8; 16];
        hex::encode_to_slice(&bytes[0..8], &mut value)
            .expect("8 bytes always encode to 16 hex characters");
        let value = CompactString::from_utf8(value).expect("hex output is always valid UTF-8");

        Ok(ContentBasedPathHash::Specified(value))
    }

    /// Output artifacts are written to a known location before being moved to their
    /// final, content-based location.
    pub fn for_output_artifact() -> ContentBasedPathHash {
        ContentBasedPathHash::OutputArtifact
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        match self {
            ContentBasedPathHash::Specified(value) => value.as_str(),
            // We deliberately make this 16 characters long so that it's the same length
            // as the content hash that will replace it.
            ContentBasedPathHash::OutputArtifact => "output_artifacts",
            ContentBasedPathHash::AqueryPlaceholder => "aquery_placeholder",
            ContentBasedPathHash::Scratch => "scratch",
            ContentBasedPathHash::RelativePathResolution => "relative_path_resolution",
            ContentBasedPathHash::DepFilesPlaceholder => "dep_files_placeholder",
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::content_hash::ContentBasedPathHash;

    #[test]
    fn test_hash() {
        assert_eq!(
            "3030303030303030",
            ContentBasedPathHash::new("00000000".as_bytes())
                .unwrap()
                .as_str(),
        );
        assert_eq!(
            "6666666666666666",
            ContentBasedPathHash::new("ffffffff".as_bytes())
                .unwrap()
                .as_str(),
        );
    }

    #[test]
    fn test_hash_too_short() {
        let res = ContentBasedPathHash::new("0000".as_bytes());
        assert!(res.is_err());
        assert!(res.unwrap_err().category_key().ends_with("NotLongEnough"));
    }

    #[test]
    fn test_hash_is_stored_inline() {
        let hash = ContentBasedPathHash::new(b"00000000").expect("valid content hash");
        let ContentBasedPathHash::Specified(value) = hash else {
            panic!("expected specified content hash");
        };
        assert!(!value.is_heap_allocated());
    }

    #[test]
    fn test_hash_for_output_artifact() {
        assert_eq!(
            "output_artifacts",
            ContentBasedPathHash::for_output_artifact().as_str()
        );
    }

    #[test]
    fn test_hash_for_aquery_placeholder() {
        assert_eq!(
            "aquery_placeholder",
            ContentBasedPathHash::AqueryPlaceholder.as_str()
        );
    }

    #[test]
    fn test_hash_for_scratch_path() {
        assert_eq!("scratch", ContentBasedPathHash::Scratch.as_str());
    }

    #[test]
    fn test_hash_for_relative_path_resolution() {
        assert_eq!(
            "relative_path_resolution",
            ContentBasedPathHash::RelativePathResolution.as_str()
        );
    }

    #[test]
    fn test_hash_for_dep_files_placeholder() {
        assert_eq!(
            "dep_files_placeholder",
            ContentBasedPathHash::DepFilesPlaceholder.as_str()
        );
    }
}
