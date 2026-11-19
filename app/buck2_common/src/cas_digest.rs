/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

mod interner;

use std::fmt;
use std::hash::Hash;
use std::hash::Hasher;
use std::io::Read;
use std::marker::PhantomData;
use std::str;

use allocative::Allocative;
use derivative::Derivative;
use derive_more::Display;
use digest::Digest;
use dupe::Clone_;
use dupe::Copy_;
use dupe::Dupe;
use dupe::Dupe_;
use num_enum::TryFromPrimitive;
use pagable::Pagable;
use sha1::Sha1;
use sha2::Sha256;
use static_interner::Intern;
use strong_hash::StrongHash;

pub use crate::cas_digest::interner::DigestInterner;
pub use crate::cas_digest::interner::DigestInternerStats;
pub use crate::cas_digest::interner::StashRecord;
pub use crate::cas_digest::interner::SweepStats;
pub use crate::cas_digest::interner::TrackedFileDigest;
pub use crate::cas_digest::interner::Verdict;
pub use crate::cas_digest::interner::file_digest_interner;
pub use crate::cas_digest::interner::file_digest_interner_stats;

/// The number of bytes required by a SHA-1 hash
pub const SHA1_SIZE: usize = 20;

/// The number of bytes required by a SHA-256 hash
pub const SHA256_SIZE: usize = 32;

/// The number of bytes required by a Blake3 hash
pub const BLAKE3_SIZE: usize = 32;

/// The bytes that make up a file digest.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Allocative, Clone, Copy, Pagable)]
pub enum RawDigest {
    // TODO: Perhaps this should be represented as a (DigestAlgorithmKind, [0;32])
    Sha1([u8; SHA1_SIZE]),
    Sha256([u8; SHA256_SIZE]),
    Blake3([u8; BLAKE3_SIZE]),
    Blake3Keyed([u8; BLAKE3_SIZE]),
}

// We consider copying 20 bytes is cheap enough not to qualify for Dupe
impl Dupe for RawDigest {}

impl RawDigest {
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Sha1(x) => x,
            Self::Sha256(x) => x,
            Self::Blake3(x) => x,
            Self::Blake3Keyed(x) => x,
        }
    }

    pub fn algorithm(&self) -> DigestAlgorithmFamily {
        match self {
            Self::Sha1(..) => DigestAlgorithmFamily::Sha1,
            Self::Sha256(..) => DigestAlgorithmFamily::Sha256,
            Self::Blake3(..) => DigestAlgorithmFamily::Blake3,
            Self::Blake3Keyed(..) => DigestAlgorithmFamily::Blake3Keyed,
        }
    }

    pub fn parse_sha1(data: &[u8]) -> Result<Self, CasDigestParseError> {
        let mut sha1 = [0; SHA1_SIZE];
        hex::decode_to_slice(data, &mut sha1).map_err(CasDigestParseError::InvalidSha1)?;
        Ok(RawDigest::Sha1(sha1))
    }

    pub fn parse_sha256(data: &[u8]) -> Result<Self, CasDigestParseError> {
        let mut sha256 = [0; SHA256_SIZE];
        hex::decode_to_slice(data, &mut sha256).map_err(CasDigestParseError::InvalidSha256)?;
        Ok(RawDigest::Sha256(sha256))
    }

    pub fn parse_blake3(data: &[u8]) -> Result<Self, CasDigestParseError> {
        let mut blake3 = [0; BLAKE3_SIZE];
        hex::decode_to_slice(data, &mut blake3).map_err(CasDigestParseError::InvalidBlake3)?;
        Ok(RawDigest::Blake3(blake3))
    }

    pub fn parse_blake3_keyed(data: &[u8]) -> Result<Self, CasDigestParseError> {
        let mut blake3 = [0; BLAKE3_SIZE];
        hex::decode_to_slice(data, &mut blake3).map_err(CasDigestParseError::InvalidBlake3)?;
        Ok(RawDigest::Blake3Keyed(blake3))
    }
}

impl fmt::Debug for RawDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for RawDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sha1(x) => fmt_hex::<SHA1_SIZE, { SHA1_SIZE * 2 }>(x, f),
            Self::Sha256(x) => fmt_hex::<SHA256_SIZE, { SHA256_SIZE * 2 }>(x, f),
            Self::Blake3(x) | Self::Blake3Keyed(x) => {
                fmt_hex::<BLAKE3_SIZE, { BLAKE3_SIZE * 2 }>(x, f)
            }
        }
    }
}

/// Hex-encodes `bytes` into a stack buffer, honoring the formatter's fill, alignment, width and
/// precision like `str` does. `HEX_LEN` must be `N * 2`; the compiler enforces it.
fn fmt_hex<const N: usize, const HEX_LEN: usize>(
    bytes: &[u8; N],
    f: &mut fmt::Formatter<'_>,
) -> fmt::Result {
    const { assert!(HEX_LEN == N * 2) };
    let mut encoded = [0u8; HEX_LEN];
    hex::encode_to_slice(bytes, &mut encoded)
        .expect("should fit: the buffer is exactly twice the digest length");
    f.pad(str::from_utf8(&encoded).expect("should be valid UTF-8: hex output is ASCII"))
}

/// The family of digest algorithm associated with a digest. This tells you what kind of digest it
/// is, but it might not be sufficient in order to actually recreate the digest. For example, this
/// could contain keyed digests, but then you wouldn't have the key. We use this to store our
/// digest kind when it's informative-only, like in our materializer state on disk.
#[derive(
    Debug,
    Display,
    Eq,
    PartialEq,
    TryFromPrimitive,
    Copy,
    Clone,
    Dupe,
    Hash,
    Allocative
)]
#[repr(u8)]
pub enum DigestAlgorithmFamily {
    #[display("SHA1")]
    Sha1,
    #[display("SHA256")]
    Sha256,
    #[display("BLAKE3")]
    Blake3,
    #[display("BLAKE3-KEYED")]
    Blake3Keyed,
}

#[derive(buck2_error::Error, Debug)]
#[error("Invalid Digest algorithm: `{0}`")]
#[buck2(tag = Input)]
pub struct InvalidDigestAlgorithmFamily(String);

impl std::str::FromStr for DigestAlgorithmFamily {
    type Err = InvalidDigestAlgorithmFamily;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "SHA1" {
            return Ok(Self::Sha1);
        }

        if s == "SHA256" {
            return Ok(Self::Sha256);
        }

        if s == "BLAKE3" {
            return Ok(Self::Blake3);
        }

        if s == "BLAKE3-KEYED" {
            return Ok(Self::Blake3Keyed);
        }

        Err(InvalidDigestAlgorithmFamily(s.to_owned()))
    }
}

/// An actual digest algorithm you can use to hash data.
#[derive(Debug, Eq, PartialEq, Copy, Clone, Dupe, Hash, Allocative, Pagable)]
pub enum DigestAlgorithm {
    Sha1,
    Sha256,
    Blake3,
    Blake3Keyed,
    Blake3KeyedTest,
}

impl DigestAlgorithm {
    fn family(self) -> DigestAlgorithmFamily {
        match self {
            Self::Sha1 => DigestAlgorithmFamily::Sha1,
            Self::Sha256 => DigestAlgorithmFamily::Sha256,
            Self::Blake3 => DigestAlgorithmFamily::Blake3,
            Self::Blake3Keyed | Self::Blake3KeyedTest => DigestAlgorithmFamily::Blake3Keyed,
        }
    }
}

#[derive(Copy, Clone, Dupe, Debug, Allocative, Hash, Eq, PartialEq, Pagable)]
pub struct CasDigestConfig {
    inner: Intern<CasDigestConfigInner>,
}

impl CasDigestConfig {
    pub fn testing_default() -> Self {
        Self {
            inner: CAS_DIGEST_CONFIG_INTERNER
                .intern(CasDigestConfigInner::new(vec![DigestAlgorithm::Sha1], None).unwrap()),
        }
    }

    /// Create a new CasDigestConfig. Values are interned so that identical configs
    /// share a single allocation.
    pub fn leak_new(
        algorithms: Vec<DigestAlgorithm>,
        preferred_source_algorithm: Option<DigestAlgorithm>,
    ) -> Result<Self, CasDigestConfigError> {
        let inner = CAS_DIGEST_CONFIG_INTERNER.intern(CasDigestConfigInner::new(
            algorithms,
            preferred_source_algorithm,
        )?);
        Ok(Self { inner })
    }

    /// Allow optimizing the empty file digest path, we do that by having the CasDigestConfig hold
    /// a cell for it (later in this stack).
    pub fn empty_file_digest(self) -> TrackedFileDigest {
        self.inner.empty_file_digest.dupe()
    }

    /// Whether `digest` is the digest of empty content under one of the configured algorithms. A
    /// size of 0 alone does not qualify: a declared digest can pair a size of 0 with the hash of
    /// other content, and that pair names nothing in a CAS.
    pub fn is_empty_digest<Kind: CasDigestKind>(self, digest: &CasDigest<Kind>) -> bool {
        digest.size() == 0 && self.inner.empty_digests.contains(digest.raw_digest())
    }

    pub fn preferred_algorithm(self) -> DigestAlgorithm {
        self.inner.preferred_algorithm
    }

    pub fn digest160(self) -> Option<DigestAlgorithm> {
        self.inner.digest160
    }

    pub fn digest256(self) -> Option<DigestAlgorithm> {
        self.inner.digest256
    }

    pub fn allows_sha1(self) -> bool {
        self.inner.digest160 == Some(DigestAlgorithm::Sha1)
    }

    pub fn allows_sha256(self) -> bool {
        self.inner.digest256 == Some(DigestAlgorithm::Sha256)
    }

    pub fn allows_blake3(self) -> bool {
        self.inner.digest256 == Some(DigestAlgorithm::Blake3)
    }

    pub fn allows_blake3_keyed(self) -> bool {
        matches!(
            self.inner.digest256,
            Some(DigestAlgorithm::Blake3Keyed | DigestAlgorithm::Blake3KeyedTest)
        )
    }

    /// The configured algorithm of `family`, if this config accepts digests of that family.
    ///
    /// This is what to hash with when the result has to compare equal to a digest that is already
    /// in hand: the preferred algorithm may be a different family from that digest's.
    pub fn algorithm_for_family(self, family: DigestAlgorithmFamily) -> Option<DigestAlgorithm> {
        let slot = match family {
            DigestAlgorithmFamily::Sha1 => self.inner.digest160,
            DigestAlgorithmFamily::Sha256
            | DigestAlgorithmFamily::Blake3
            | DigestAlgorithmFamily::Blake3Keyed => self.inner.digest256,
        };
        slot.filter(|algorithm| algorithm.family() == family)
    }

    /// Access the config for source files. Note that there is no method to go back to the
    /// non-source config.
    pub fn source_files_config(self) -> Self {
        match &self.inner.source {
            SourceFilesConfig::UseSelf => self,
            SourceFilesConfig::UseThis(other) => Self { inner: *other },
        }
    }
}

impl fmt::Display for CasDigestConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CasDigestConfig(preferred = {}, source preferred = {})",
            self.preferred_algorithm().family(),
            self.source_files_config().preferred_algorithm().family()
        )
    }
}

static_interner::interner!(
    CAS_DIGEST_CONFIG_INTERNER,
    buck2_hash::BuckHasher,
    CasDigestConfigInner
);

#[derive(Debug, Allocative, Hash, Eq, PartialEq, Pagable)]
struct CasDigestConfigInner {
    /// The algorithm we use for non-source files, action digests, etc.
    preferred_algorithm: DigestAlgorithm,
    digest160: Option<DigestAlgorithm>,
    digest256: Option<DigestAlgorithm>,
    empty_file_digest: TrackedFileDigest,
    /// The digest of empty content under each configured algorithm.
    empty_digests: Vec<RawDigest>,
    /// A potentially different configuration to use when digesting source files.
    source: SourceFilesConfig,
}

#[derive(Debug, Allocative, Hash, Eq, PartialEq, Pagable)]
enum SourceFilesConfig {
    UseSelf,
    UseThis(Intern<CasDigestConfigInner>),
}

impl CasDigestConfigInner {
    /// Initialize a CasDigestConfigInner. The algorithms should be listed in decreasing order of
    /// preference.
    fn new(
        algorithms: Vec<DigestAlgorithm>,
        preferred_source_algorithm: Option<DigestAlgorithm>,
    ) -> Result<Self, CasDigestConfigError> {
        let preferred_algorithm = *algorithms
            .first()
            .ok_or(CasDigestConfigError::NotConfigured)?;

        let preferred_source_algorithm = preferred_source_algorithm
            .map(|a| {
                if algorithms.contains(&a) {
                    Ok(a)
                } else {
                    Err(CasDigestConfigError::InvalidPreferredSourceAlgorithm)
                }
            })
            .transpose()?;

        let empty_digests = algorithms
            .iter()
            .map(|algo| {
                CasDigestData::digester_for_algorithm(*algo)
                    .finalize()
                    .digest
            })
            .collect();

        let mut digest160 = None;
        let mut digest256 = None;

        for algo in algorithms {
            let slot = match algo {
                DigestAlgorithm::Sha1 => &mut digest160,
                DigestAlgorithm::Sha256 => &mut digest256,
                DigestAlgorithm::Blake3 => &mut digest256,
                DigestAlgorithm::Blake3Keyed | DigestAlgorithm::Blake3KeyedTest => &mut digest256,
            };

            if let Some(slot) = &slot {
                return Err(CasDigestConfigError::Conflict(*slot, algo));
            }

            *slot = Some(algo);
        }

        let empty_file_digest = TrackedFileDigest::from_parts(
            CasDigest::from_content_for_algorithm(&[], preferred_algorithm),
            0,
        );

        let source = match preferred_source_algorithm {
            Some(algo) => SourceFilesConfig::UseThis(
                CAS_DIGEST_CONFIG_INTERNER.intern(Self::new(vec![algo], None)?),
            ),
            None => SourceFilesConfig::UseSelf,
        };

        Ok(Self {
            preferred_algorithm,
            digest160,
            digest256,
            empty_file_digest,
            empty_digests,
            source,
        })
    }
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Tier0)]
pub enum CasDigestConfigError {
    #[error("At least one algorithm must be enabled")]
    NotConfigured,
    #[error("The preferred source algorithm must be in the algorithms list")]
    InvalidPreferredSourceAlgorithm,
    #[error("Two algorithms were enabled for the same size: `{}` and `{}`", .0.family(), .1.family())]
    Conflict(DigestAlgorithm, DigestAlgorithm),
}

pub struct DataDigester {
    variant: DigesterVariant,
    size: u64,
}

pub struct Digester<Kind: CasDigestKind> {
    data: DataDigester,
    kind: PhantomData<Kind>,
}

enum DigesterVariant {
    Sha1(Sha1),
    Sha256(Sha256),
    Blake3(Box<blake3::Hasher>),      // This is unusually large.
    Blake3Keyed(Box<blake3::Hasher>), // Same as above
}

impl DataDigester {
    pub fn update(&mut self, data: &[u8]) {
        // Explicit dynamic dispatch because we need to match on which variant it was.
        match &mut self.variant {
            DigesterVariant::Sha1(h) => {
                h.update(data);
            }
            DigesterVariant::Sha256(h) => {
                h.update(data);
            }
            DigesterVariant::Blake3(h) => {
                h.update(data);
            }
            DigesterVariant::Blake3Keyed(h) => {
                h.update(data);
            }
        };

        self.size += data.len() as u64;
    }

    pub fn finalize(self) -> CasDigestData {
        match self.variant {
            DigesterVariant::Sha1(h) => CasDigestData::new_sha1(h.finalize().into(), self.size),
            DigesterVariant::Sha256(h) => CasDigestData::new_sha256(h.finalize().into(), self.size),
            DigesterVariant::Blake3(h) => CasDigestData::new_blake3(h.finalize().into(), self.size),
            DigesterVariant::Blake3Keyed(h) => {
                CasDigestData::new_blake3_keyed(h.finalize().into(), self.size)
            }
        }
    }

    pub fn algorithm(&self) -> DigestAlgorithmFamily {
        match &self.variant {
            DigesterVariant::Sha1(..) => DigestAlgorithmFamily::Sha1,
            DigesterVariant::Sha256(..) => DigestAlgorithmFamily::Sha256,
            DigesterVariant::Blake3(..) => DigestAlgorithmFamily::Blake3,
            DigesterVariant::Blake3Keyed(..) => DigestAlgorithmFamily::Blake3Keyed,
        }
    }

    pub fn bytes_read(&self) -> u64 {
        self.size
    }
}

impl<Kind: CasDigestKind> Digester<Kind> {
    pub fn update(&mut self, data: &[u8]) {
        self.data.update(data);
    }

    pub fn finalize(self) -> CasDigest<Kind> {
        CasDigest {
            data: self.data.finalize(),
            kind: PhantomData,
        }
    }

    pub fn algorithm(&self) -> DigestAlgorithmFamily {
        self.data.algorithm()
    }

    pub fn bytes_read(&self) -> u64 {
        self.data.bytes_read()
    }
}

/// Separate struct to allow us to use  `repr(transparent)` below and guarantee an identical
/// layout.
#[derive(
    Display, PartialEq, Eq, PartialOrd, Ord, Hash, Allocative, Clone, Dupe, Copy, Pagable
)]
#[display("{}:{}", digest, size)]
pub struct CasDigestData {
    size: u64,
    digest: RawDigest,
}

impl CasDigestData {
    fn new(digest: RawDigest, size: u64) -> Self {
        CasDigestData { size, digest }
    }

    pub fn new_sha1(sha1: [u8; SHA1_SIZE], size: u64) -> Self {
        Self::new(RawDigest::Sha1(sha1), size)
    }

    pub fn new_sha256(sha256: [u8; SHA256_SIZE], size: u64) -> Self {
        Self::new(RawDigest::Sha256(sha256), size)
    }

    pub fn new_blake3(blake3: [u8; BLAKE3_SIZE], size: u64) -> Self {
        Self::new(RawDigest::Blake3(blake3), size)
    }

    pub fn new_blake3_keyed(blake3: [u8; BLAKE3_SIZE], size: u64) -> Self {
        Self::new(RawDigest::Blake3Keyed(blake3), size)
    }

    pub fn digester(config: CasDigestConfig) -> DataDigester {
        Self::digester_for_algorithm(config.preferred_algorithm())
    }

    pub fn digester_for_algorithm(algorithm: DigestAlgorithm) -> DataDigester {
        let variant = match algorithm {
            DigestAlgorithm::Sha1 => DigesterVariant::Sha1(Sha1::new()),
            DigestAlgorithm::Sha256 => DigesterVariant::Sha256(Sha256::new()),
            DigestAlgorithm::Blake3 => DigesterVariant::Blake3(Box::new(blake3::Hasher::new())),
            DigestAlgorithm::Blake3Keyed => {
                #[cfg(fbcode_build)]
                {
                    DigesterVariant::Blake3Keyed(Box::new(blake3::Hasher::new_keyed(
                        blake3_constants::BLAKE3_HASH_KEY,
                    )))
                }
                #[cfg(not(fbcode_build))]
                {
                    panic!("Blake3Keyed is not supported in the open source build")
                }
            }
            DigestAlgorithm::Blake3KeyedTest => {
                static TEST_KEY: [u8; 32] = [0; 32];
                DigesterVariant::Blake3Keyed(Box::new(blake3::Hasher::new_keyed(&TEST_KEY)))
            }
        };

        DataDigester { variant, size: 0 }
    }

    pub fn raw_digest(&self) -> &RawDigest {
        &self.digest
    }
}

#[derive(Display, Derivative, Allocative, Clone_, Dupe_, Copy_, Pagable)]
#[allocative(bound = "")]
#[derivative(PartialEq, Eq, PartialOrd, Ord, Hash)]
#[display("{}", data)]
#[repr(transparent)]
pub struct CasDigest<Kind: CasDigestKind> {
    data: CasDigestData,
    #[derivative(
        Hash = "ignore",
        PartialEq = "ignore",
        PartialOrd = "ignore",
        Ord = "ignore"
    )]
    kind: PhantomData<Kind>,
}

impl<Kind: CasDigestKind> fmt::Debug for CasDigest<Kind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl<Kind: CasDigestKind> CasDigest<Kind> {
    pub fn from_digest_bytes(
        kind: DigestAlgorithmFamily,
        digest: &[u8],
        size: u64,
    ) -> buck2_error::Result<Self> {
        Ok(match kind {
            DigestAlgorithmFamily::Sha1 => Self::new_sha1(digest.try_into()?, size),
            DigestAlgorithmFamily::Sha256 => Self::new_sha256(digest.try_into()?, size),
            DigestAlgorithmFamily::Blake3 => Self::new_blake3(digest.try_into()?, size),
            DigestAlgorithmFamily::Blake3Keyed => Self::new_blake3_keyed(digest.try_into()?, size),
        })
    }

    pub fn new(digest: RawDigest, size: u64) -> Self {
        Self {
            data: CasDigestData { size, digest },
            kind: PhantomData,
        }
    }

    pub fn new_sha1(sha1: [u8; SHA1_SIZE], size: u64) -> Self {
        Self::new(RawDigest::Sha1(sha1), size)
    }

    pub fn new_sha256(sha256: [u8; SHA256_SIZE], size: u64) -> Self {
        Self::new(RawDigest::Sha256(sha256), size)
    }

    pub fn new_blake3(blake3: [u8; BLAKE3_SIZE], size: u64) -> Self {
        Self::new(RawDigest::Blake3(blake3), size)
    }

    pub fn new_blake3_keyed(blake3: [u8; BLAKE3_SIZE], size: u64) -> Self {
        Self::new(RawDigest::Blake3Keyed(blake3), size)
    }

    pub fn raw_digest(&self) -> &RawDigest {
        self.data.raw_digest()
    }

    pub fn size(&self) -> u64 {
        self.data.size
    }

    /// A tiny representation of this digest, useful for logging when the full sha1 presentation is
    /// too expensive.
    pub fn tiny_digest(&self) -> TinyDigest<'_, Kind> {
        TinyDigest { of: self }
    }

    pub fn parse_digest(
        s: &str,
        config: CasDigestConfig,
    ) -> Result<(Self, DigestAlgorithm), CasDigestParseError> {
        let (digest, size) = s
            .split_once(':')
            .ok_or(CasDigestParseError::MissingSizeSeparator)?;

        let (digest, algo) = CasDigest::<Kind>::parse_digest_without_size(digest, config)?;
        let size = size.parse().map_err(CasDigestParseError::InvalidSize)?;

        Ok((Self::new(digest, size), algo))
    }

    pub fn parse_digest_without_size(
        data: &str,
        config: CasDigestConfig,
    ) -> Result<(RawDigest, DigestAlgorithm), CasDigestParseError> {
        let algo = if data.len() == 40 {
            config.digest160()
        } else if data.len() == 64 {
            config.digest256()
        } else {
            None
        };

        let algo = algo.ok_or(CasDigestParseError::UnsupportedDigest(data.len()))?;

        let digest = match algo {
            DigestAlgorithm::Sha1 => RawDigest::parse_sha1(data.as_bytes()),
            DigestAlgorithm::Sha256 => RawDigest::parse_sha256(data.as_bytes()),
            DigestAlgorithm::Blake3 => RawDigest::parse_blake3(data.as_bytes()),
            DigestAlgorithm::Blake3Keyed | DigestAlgorithm::Blake3KeyedTest => {
                RawDigest::parse_blake3_keyed(data.as_bytes())
            }
        }?;

        Ok((digest, algo))
    }

    /// Return the digest of an empty string
    pub fn empty(config: CasDigestConfig) -> Self {
        Self::from_content(&[], config)
    }

    pub fn digester(config: CasDigestConfig) -> Digester<Kind> {
        Self::digester_for_algorithm(config.preferred_algorithm())
    }

    pub fn digester_for_algorithm(algorithm: DigestAlgorithm) -> Digester<Kind> {
        Digester {
            data: CasDigestData::digester_for_algorithm(algorithm),
            kind: PhantomData,
        }
    }

    pub fn from_content(bytes: &[u8], config: CasDigestConfig) -> Self {
        Self::from_content_for_algorithm(bytes, config.preferred_algorithm())
    }

    pub fn from_content_for_algorithm(bytes: &[u8], algorithm: DigestAlgorithm) -> Self {
        let mut digester = Self::digester_for_algorithm(algorithm);
        digester.update(bytes);
        digester.finalize()
    }

    pub fn from_reader<R: Read>(reader: R, config: CasDigestConfig) -> buck2_error::Result<Self> {
        Self::from_reader_for_algorithm(reader, config.preferred_algorithm())
    }

    pub fn from_reader_for_algorithm<R: Read>(
        mut reader: R,
        algorithm: DigestAlgorithm,
    ) -> buck2_error::Result<Self> {
        let mut digester = Self::digester_for_algorithm(algorithm);

        // Buffer size chosen based on benchmarks at D26176645
        // Also optimal for Blake3's SIMD implementation.
        let mut buffer = [0; 16 * 1024];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digester.update(&buffer[..count]);
        }

        Ok(digester.finalize())
    }

    pub fn coerce<NewKind: CasDigestKind>(self) -> CasDigest<NewKind> {
        CasDigest {
            data: self.data,
            kind: PhantomData,
        }
    }
}

/// What a [`CasDigest`] is a digest of. Phantom: it keeps digests of files, actions and dep files
/// from being passed for one another, and nothing else.
pub trait CasDigestKind: Sized + 'static {}

#[derive(Display)]
#[display("{}", hex::encode(&of.raw_digest().as_bytes()[0..4]))]
pub struct TinyDigest<'a, Kind: CasDigestKind> {
    of: &'a CasDigest<Kind>,
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = InvalidDigest)]
pub enum CasDigestParseError {
    #[error("The digest is missing a size separator, it should look like `HASH:SIZE`")]
    MissingSizeSeparator,

    #[error("The digest part of the CAS digest is not a valid SHA1 digest")]
    InvalidSha1(#[source] hex::FromHexError),

    #[error("The digest part of the CAS digest is not a valid SHA256 digest")]
    InvalidSha256(#[source] hex::FromHexError),

    #[error("The digest part of the CAS digest is not a valid BLAKE3 hash")]
    InvalidBlake3(#[source] hex::FromHexError),

    #[error("The digest size ({} chars) does not correspond to a supported digest size", .0)]
    UnsupportedDigest(usize),

    #[error("The size part of the CAS digest is invalid")]
    InvalidSize(#[source] std::num::ParseIntError),
}

impl<Kind: CasDigestKind> StrongHash for CasDigest<Kind> {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.raw_digest().as_bytes().strong_hash(state);
        self.size().strong_hash(state);
    }
}

pub mod testing {
    use super::*;

    pub fn sha1_sha256() -> CasDigestConfig {
        CasDigestConfig {
            inner: CAS_DIGEST_CONFIG_INTERNER.intern(
                CasDigestConfigInner::new(
                    vec![DigestAlgorithm::Sha1, DigestAlgorithm::Sha256],
                    None,
                )
                .unwrap(),
            ),
        }
    }

    pub fn sha1_blake3() -> CasDigestConfig {
        CasDigestConfig {
            inner: CAS_DIGEST_CONFIG_INTERNER.intern(
                CasDigestConfigInner::new(
                    vec![DigestAlgorithm::Sha1, DigestAlgorithm::Blake3],
                    None,
                )
                .unwrap(),
            ),
        }
    }

    pub fn sha256_sha1() -> CasDigestConfig {
        CasDigestConfig {
            inner: CAS_DIGEST_CONFIG_INTERNER.intern(
                CasDigestConfigInner::new(
                    vec![DigestAlgorithm::Sha256, DigestAlgorithm::Sha1],
                    None,
                )
                .unwrap(),
            ),
        }
    }

    pub fn sha1() -> CasDigestConfig {
        CasDigestConfig {
            inner: CAS_DIGEST_CONFIG_INTERNER
                .intern(CasDigestConfigInner::new(vec![DigestAlgorithm::Sha1], None).unwrap()),
        }
    }

    pub fn sha256() -> CasDigestConfig {
        CasDigestConfig {
            inner: CAS_DIGEST_CONFIG_INTERNER
                .intern(CasDigestConfigInner::new(vec![DigestAlgorithm::Sha256], None).unwrap()),
        }
    }

    pub fn blake3() -> CasDigestConfig {
        CasDigestConfig {
            inner: CAS_DIGEST_CONFIG_INTERNER
                .intern(CasDigestConfigInner::new(vec![DigestAlgorithm::Blake3], None).unwrap()),
        }
    }

    pub fn blake3_keyed() -> CasDigestConfig {
        CasDigestConfig {
            inner: CAS_DIGEST_CONFIG_INTERNER.intern(
                CasDigestConfigInner::new(vec![DigestAlgorithm::Blake3KeyedTest], None).unwrap(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_ops::metadata::FileDigestKind;

    #[test]
    fn test_raw_digest_display_all_algorithms() {
        // Cover every byte value, including leading zeroes and high-bit bytes.
        for start in (0u8..=224).step_by(32) {
            let bytes = std::array::from_fn(|i| start + i as u8);
            let digests = [
                RawDigest::Sha1(bytes[..SHA1_SIZE].try_into().unwrap()),
                RawDigest::Sha256(bytes),
                RawDigest::Blake3(bytes),
                RawDigest::Blake3Keyed(bytes),
            ];
            for digest in digests {
                assert_eq!(hex::encode(digest.as_bytes()), digest.to_string());
            }
        }
    }

    #[test]
    fn test_raw_digest_display_format_options() {
        let digests = [
            RawDigest::Sha1([0x9a; SHA1_SIZE]),
            RawDigest::Sha256([0x9a; SHA256_SIZE]),
        ];
        for digest in digests {
            // Width, fill, alignment and precision apply exactly as they would to the hex `str`.
            let hex = hex::encode(digest.as_bytes());
            assert_eq!(format!("{hex:>80}"), format!("{digest:>80}"));
            assert_eq!(format!("{hex:_<80}"), format!("{digest:_<80}"));
            assert_eq!(format!("{hex:^80}"), format!("{digest:^80}"));
            assert_eq!(format!("{hex:.7}"), format!("{digest:.7}"));
        }

        let digest = RawDigest::Sha256([0x9a; SHA256_SIZE]);
        assert_eq!(
            format!("{:>16}{}", "", "9a".repeat(SHA256_SIZE)),
            format!("{digest:>80}")
        );
        assert_eq!("9a9a9a9", format!("{digest:.7}"));
    }

    #[test]
    fn test_digest_from_str() {
        let s = "0000000000000000000000000000000000000000:123";
        let config = CasDigestConfig::testing_default();
        assert_eq!(
            CasDigest::<FileDigestKind>::parse_digest(s, config)
                .unwrap()
                .0
                .to_string(),
            s
        );
    }

    #[test]
    fn test_digest_from_reader() {
        let content = &b"foo"[..];

        assert_eq!(
            CasDigest::<FileDigestKind>::from_reader_for_algorithm(content, DigestAlgorithm::Sha1)
                .unwrap()
                .to_string(),
            "0beec7b5ea3f0fdbc95d0dd47f3c5bc275da8a33:3"
        );
        assert_eq!(
            CasDigest::<FileDigestKind>::from_reader_for_algorithm(
                content,
                DigestAlgorithm::Sha256
            )
            .unwrap()
            .to_string(),
            "2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae:3"
        );
        assert_eq!(
            CasDigest::<FileDigestKind>::from_reader_for_algorithm(
                content,
                DigestAlgorithm::Blake3
            )
            .unwrap()
            .to_string(),
            "04e0bb39f30b1a3feb89f536c93be15055482df748674b00d26e5a75777702e9:3"
        );
    }

    #[test]
    fn test_digest_from_content() {
        let content = &b"foo"[..];

        assert_eq!(
            CasDigest::<FileDigestKind>::from_content_for_algorithm(content, DigestAlgorithm::Sha1)
                .to_string(),
            "0beec7b5ea3f0fdbc95d0dd47f3c5bc275da8a33:3"
        );
        assert_eq!(
            CasDigest::<FileDigestKind>::from_content_for_algorithm(
                content,
                DigestAlgorithm::Sha256
            )
            .to_string(),
            "2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae:3"
        );
        assert_eq!(
            CasDigest::<FileDigestKind>::from_content_for_algorithm(
                content,
                DigestAlgorithm::Blake3
            )
            .to_string(),
            "04e0bb39f30b1a3feb89f536c93be15055482df748674b00d26e5a75777702e9:3"
        );
    }

    #[test]
    fn test_preferred_algorithm() {
        let content = &b"foo"[..];

        assert_eq!(
            CasDigest::<FileDigestKind>::from_content(content, testing::sha1(),).to_string(),
            "0beec7b5ea3f0fdbc95d0dd47f3c5bc275da8a33:3"
        );
        assert_eq!(
            CasDigest::<FileDigestKind>::from_content(content, testing::sha256(),).to_string(),
            "2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae:3"
        );
        assert_eq!(
            CasDigest::<FileDigestKind>::from_content(content, testing::blake3(),).to_string(),
            "04e0bb39f30b1a3feb89f536c93be15055482df748674b00d26e5a75777702e9:3"
        );
    }

    #[test]
    fn test_parse_digest() {
        let sha1 = CasDigest::<FileDigestKind>::parse_digest(
            "0beec7b5ea3f0fdbc95d0dd47f3c5bc275da8a33:3",
            testing::sha256_sha1(),
        )
        .unwrap()
        .0;
        assert_eq!(sha1.raw_digest().algorithm(), DigestAlgorithmFamily::Sha1);
        assert_eq!(
            sha1.to_string(),
            "0beec7b5ea3f0fdbc95d0dd47f3c5bc275da8a33:3"
        );

        let sha256 = CasDigest::<FileDigestKind>::parse_digest(
            "2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae:3",
            testing::sha1_sha256(),
        )
        .unwrap()
        .0;
        assert_eq!(
            sha256.raw_digest().algorithm(),
            DigestAlgorithmFamily::Sha256
        );
        assert_eq!(
            sha256.to_string(),
            "2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae:3"
        );

        let blake3 = CasDigest::<FileDigestKind>::parse_digest(
            "04e0bb39f30b1a3feb89f536c93be15055482df748674b00d26e5a75777702e9:3",
            testing::sha1_blake3(),
        )
        .unwrap()
        .0;
        assert_eq!(
            blake3.raw_digest().algorithm(),
            DigestAlgorithmFamily::Blake3
        );
        assert_eq!(
            blake3.to_string(),
            "04e0bb39f30b1a3feb89f536c93be15055482df748674b00d26e5a75777702e9:3"
        );

        let blake3_keyed = CasDigest::<FileDigestKind>::parse_digest(
            "04e0bb39f30b1a3feb89f536c93be15055482df748674b00d26e5a75777702e9:3",
            testing::blake3_keyed(),
        )
        .unwrap()
        .0;
        assert_eq!(
            blake3_keyed.raw_digest().algorithm(),
            DigestAlgorithmFamily::Blake3Keyed
        );
        assert_eq!(
            blake3_keyed.to_string(),
            "04e0bb39f30b1a3feb89f536c93be15055482df748674b00d26e5a75777702e9:3"
        );
    }

    #[test]
    fn test_digest_algorithm_kind_roundtrip() {
        for v in [
            DigestAlgorithmFamily::Sha1,
            DigestAlgorithmFamily::Sha256,
            DigestAlgorithmFamily::Blake3,
            DigestAlgorithmFamily::Blake3Keyed,
        ] {
            assert_eq!(v, v.to_string().parse().unwrap());
        }
    }
}
