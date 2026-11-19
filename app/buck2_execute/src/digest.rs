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

use buck2_common::cas_digest::CasDigest;
use buck2_common::cas_digest::CasDigestKind;
use buck2_common::cas_digest::CasDigestParseError;
use buck2_common::cas_digest::DigestAlgorithm;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use remote_execution::Digest;
use remote_execution::TDigest;

use crate::digest_config::DigestConfig;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = InvalidDigest)]
pub enum DigestConversionError {
    #[error("Error parsing digest: `{}`", digest)]
    ParseError {
        digest: String,

        #[source]
        error: CasDigestParseError,
    },
}

pub trait CasDigestFromReExt: Sized {
    fn from_re_with_algo(
        x: &TDigest,
        digest_config: DigestConfig,
    ) -> Result<(Self, DigestAlgorithm), DigestConversionError>;

    /// Parse a digest and return the algorithm it used
    fn from_grpc_with_algo(
        x: &Digest,
        digest_config: DigestConfig,
    ) -> Result<(Self, DigestAlgorithm), DigestConversionError>;

    fn from_re(x: &TDigest, digest_config: DigestConfig) -> Result<Self, DigestConversionError> {
        Self::from_re_with_algo(x, digest_config).map(|(d, _a)| d)
    }

    fn from_grpc(x: &Digest, digest_config: DigestConfig) -> Result<Self, DigestConversionError> {
        Self::from_grpc_with_algo(x, digest_config).map(|(d, _a)| d)
    }
}

pub trait CasDigestToReExt {
    fn to_re(&self) -> TDigest;
    fn to_grpc(&self) -> Digest;
}

impl<Kind: CasDigestKind> CasDigestFromReExt for CasDigest<Kind> {
    fn from_re_with_algo(
        digest: &TDigest,
        digest_config: DigestConfig,
    ) -> Result<(Self, DigestAlgorithm), DigestConversionError> {
        let (ret, algo) =
            Self::parse_digest_without_size(&digest.hash, digest_config.cas_digest_config())
                .map_err(|error| DigestConversionError::ParseError {
                    digest: digest.to_string(),
                    error,
                })?;
        Ok((Self::new(ret, digest.size_in_bytes as u64), algo))
    }

    fn from_grpc_with_algo(
        digest: &Digest,
        digest_config: DigestConfig,
    ) -> Result<(Self, DigestAlgorithm), DigestConversionError> {
        let (ret, algo) =
            Self::parse_digest_without_size(&digest.hash, digest_config.cas_digest_config())
                .map_err(|error| DigestConversionError::ParseError {
                    digest: format!("{}:{}", digest.hash, digest.size_bytes),
                    error,
                })?;

        Ok((Self::new(ret, digest.size_bytes as u64), algo))
    }
}

pub trait CasDigestConversionResultExt {
    fn as_display(&self) -> &dyn fmt::Display;
}

impl<Kind: CasDigestKind> CasDigestConversionResultExt
    for Result<CasDigest<Kind>, DigestConversionError>
{
    fn as_display(&self) -> &dyn fmt::Display {
        match self {
            Self::Ok(v) => v as _,
            Self::Err(DigestConversionError::ParseError { digest, .. }) => digest as _,
        }
    }
}

impl CasDigestToReExt for TrackedFileDigest {
    fn to_re(&self) -> TDigest {
        self.data().to_re()
    }

    fn to_grpc(&self) -> Digest {
        self.data().to_grpc()
    }
}

impl<Kind: CasDigestKind> CasDigestToReExt for CasDigest<Kind> {
    fn to_re(&self) -> TDigest {
        TDigest {
            hash: self.raw_digest().to_string(),
            size_in_bytes: self.size() as i64,
            ..Default::default()
        }
    }

    fn to_grpc(&self) -> Digest {
        Digest {
            hash: self.raw_digest().to_string(),
            size_bytes: self.size() as i64,
        }
    }
}
