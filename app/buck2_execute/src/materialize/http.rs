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
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use allocative::Allocative;
use buck2_common::cas_digest::CasDigestConfig;
use buck2_common::cas_digest::DigestAlgorithmFamily;
use buck2_common::cas_digest::SHA1_SIZE;
use buck2_common::cas_digest::SHA256_SIZE;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_error::BuckErrorContext;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_http::HttpClient;
use buck2_http::retries::HttpError;
use buck2_http::retries::HttpErrorForRetry;
use buck2_http::retries::IntoBuck2Error;
use buck2_http::retries::http_retry;
use bytes::Bytes;
use digest::DynDigest;
use dupe::Dupe;
use futures::StreamExt;
use futures::stream::Stream;
use hyper::Response;
use pagable::Pagable;
use sha1::Digest;
use sha1::Sha1;
use sha2::Sha256;
use sha2::Sha512;
use smallvec::SmallVec;

use crate::digest_config::DigestConfig;

type DynHasher = Box<dyn DynDigest + Send>;

/// Length in bytes of a SHA-512 digest.
const SHA512_SIZE: usize = 64;

/// A hash algorithm a download can be checked against. It is the argument name of
/// `download_file` (`sha256 = ...`) that carries the digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Dupe, Allocative, Pagable)]
pub enum ChecksumAlgorithm {
    Sha1,
    Sha256,
    Sha512,
}

/// What it takes to check a download against one [`ChecksumAlgorithm`]: its name as a rule author
/// spells it, the length of its digest, and how to compute it.
///
/// `HASH_ALGORITHMS` is the single place that knows how to compute each algorithm. Supporting
/// another one (for example `sha3_256`) is one more `ChecksumAlgorithm` variant plus its entry
/// there and the matching named argument of `download_file`.
struct HashAlgorithm {
    algorithm: ChecksumAlgorithm,
    name: &'static str,
    /// Length of the digest in bytes (the hex form is twice as long).
    digest_len: usize,
    new_hasher: fn() -> DynHasher,
    /// The digest-config algorithm that computes this same hash, if there is one. A download is
    /// then checked with the digest it computes for the CAS anyway instead of hashing twice.
    digest_family: Option<DigestAlgorithmFamily>,
}

static HASH_ALGORITHMS: &[HashAlgorithm] = &[
    HashAlgorithm {
        algorithm: ChecksumAlgorithm::Sha1,
        name: "sha1",
        digest_len: SHA1_SIZE,
        new_hasher: || Box::new(Sha1::new()) as DynHasher,
        digest_family: Some(DigestAlgorithmFamily::Sha1),
    },
    HashAlgorithm {
        algorithm: ChecksumAlgorithm::Sha256,
        name: "sha256",
        digest_len: SHA256_SIZE,
        new_hasher: || Box::new(Sha256::new()) as DynHasher,
        digest_family: Some(DigestAlgorithmFamily::Sha256),
    },
    HashAlgorithm {
        algorithm: ChecksumAlgorithm::Sha512,
        name: "sha512",
        digest_len: SHA512_SIZE,
        new_hasher: || Box::new(Sha512::new()) as DynHasher,
        digest_family: None,
    },
];

impl ChecksumAlgorithm {
    fn spec(self) -> &'static HashAlgorithm {
        HASH_ALGORITHMS
            .iter()
            .find(|a| a.algorithm == self)
            .expect("every `ChecksumAlgorithm` has an entry in `HASH_ALGORITHMS`")
    }
}

/// One (algorithm, digest) pair of a [`Checksum`].
#[derive(Debug, Clone, Allocative, Pagable)]
struct ChecksumEntry {
    algorithm: ChecksumAlgorithm,
    /// Lowercase hex.
    hex: Arc<str>,
}

/// The checksums a download must match: at least one (algorithm, digest) pair, at most one per
/// algorithm. Built by [`Checksum::from_pairs`], which checks the digests.
#[derive(Debug, Clone, Dupe, Allocative, Pagable)]
pub struct Checksum(Arc<[ChecksumEntry]>);

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Input)]
enum DownloadFileError {
    #[error("Must pass in at least one checksum (e.g. `sha1 = ...`)")]
    MissingChecksum,
    #[error("Checksum algorithm `{0}` given more than once")]
    DuplicateAlgorithm(&'static str),
    #[error("Invalid digest for `{digest_type}` argument, expected length of {expected_len} but got {}, digest `{digest}`", digest.len())]
    InvalidDigestLength {
        digest: String,
        expected_len: usize,
        digest_type: &'static str,
    },
    #[error(
        "Invalid digest for `{digest_type}` argument, expected 0-9 a-z hex characters, but got `{bad_char}`, digest `{digest}`"
    )]
    InvalidDigestCharacter {
        digest: String,
        bad_char: char,
        digest_type: &'static str,
    },
}

impl Checksum {
    /// Builds a checksum from (algorithm, hex digest) pairs. Refuses an algorithm given twice, a
    /// digest of the wrong length or with non-hex characters, and an empty list. The content has
    /// to match every pair.
    pub fn from_pairs(pairs: &[(ChecksumAlgorithm, &str)]) -> buck2_error::Result<Self> {
        fn is_hex_char(x: char) -> bool {
            x.is_ascii_digit() || ('a'..='f').contains(&x.to_ascii_lowercase())
        }

        let mut validated: Vec<ChecksumEntry> = Vec::new();
        for &(algorithm, digest) in pairs {
            let spec = algorithm.spec();
            if validated.iter().any(|e| e.algorithm == algorithm) {
                return Err(DownloadFileError::DuplicateAlgorithm(spec.name).into());
            }
            let expected_len = spec.digest_len * 2;
            if digest.len() != expected_len {
                return Err(DownloadFileError::InvalidDigestLength {
                    digest: digest.to_owned(),
                    expected_len,
                    digest_type: spec.name,
                }
                .into());
            }
            if let Some(bad_char) = digest.chars().find(|x| !is_hex_char(*x)) {
                return Err(DownloadFileError::InvalidDigestCharacter {
                    digest: digest.to_owned(),
                    bad_char,
                    digest_type: spec.name,
                }
                .into());
            }
            validated.push(ChecksumEntry {
                algorithm,
                hex: Arc::from(digest.to_ascii_lowercase()),
            });
        }

        if validated.is_empty() {
            return Err(DownloadFileError::MissingChecksum.into());
        }
        Ok(Self(Arc::from(validated)))
    }

    /// The digest this checksum has for `algorithm`, as lowercase hex.
    pub fn get(&self, algorithm: ChecksumAlgorithm) -> Option<&str> {
        self.0
            .iter()
            .find(|e| e.algorithm == algorithm)
            .map(|e| &*e.hex)
    }

    pub fn sha1(&self) -> Option<&str> {
        self.get(ChecksumAlgorithm::Sha1)
    }

    pub fn sha256(&self) -> Option<&str> {
        self.get(ChecksumAlgorithm::Sha256)
    }

    pub fn sha512(&self) -> Option<&str> {
        self.get(ChecksumAlgorithm::Sha512)
    }

    /// Every (algorithm, hex digest) pair the content has to match.
    fn pairs(&self) -> impl Iterator<Item = (ChecksumAlgorithm, &str)> {
        self.0.iter().map(|e| (e.algorithm, &*e.hex))
    }

    /// A checksum with no validation of the digests, to test what verification does with a
    /// wrong or unusual one.
    #[cfg(test)]
    fn unvalidated(entries: &[(ChecksumAlgorithm, &str)]) -> Self {
        Self(
            entries
                .iter()
                .map(|&(algorithm, hex)| ChecksumEntry {
                    algorithm,
                    hex: Arc::from(hex),
                })
                .collect(),
        )
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Http)]
enum HttpHeadError {
    #[error("Error performing http_head request")]
    Client(#[source] HttpError),
}

impl From<HttpError> for HttpHeadError {
    fn from(e: HttpError) -> Self {
        Self::Client(e)
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Http)]
enum HttpDownloadError {
    #[error("Error performing http_download request")]
    Client(#[source] HttpError),

    #[error(
        "Invalid {digest_kind} digest. Expected {expected}, got {obtained}. URL: {url}. {debug}"
    )]
    #[buck2(input)]
    InvalidChecksum {
        digest_kind: &'static str,
        expected: String,
        obtained: String,
        url: String,
        debug: Box<MaybeResponseDebugInfo>,
    },

    #[error(
        "Received invalid {kind} digest from {url}; perhaps this is not allowed on vpnless?. Expected {want}, got {got}. {debug}"
    )]
    MaybeNotAllowedOnVpnless {
        kind: &'static str,
        want: String,
        got: String,
        url: String,
        debug: Box<MaybeResponseDebugInfo>,
    },

    #[error(transparent)]
    IoError(buck2_error::Error),
}

impl HttpDownloadError {
    fn into_final(mut self) -> Self {
        match &mut self {
            Self::Client(..) | Self::IoError(..) => {}
            Self::InvalidChecksum { debug, .. } | Self::MaybeNotAllowedOnVpnless { debug, .. } => {
                debug.is_final = true;
            }
        }

        self
    }
}

impl From<HttpError> for HttpDownloadError {
    fn from(e: HttpError) -> Self {
        Self::Client(e)
    }
}

impl HttpErrorForRetry for HttpHeadError {
    fn is_retryable(&self) -> bool {
        match self {
            Self::Client(e) => e.is_retryable(),
        }
    }
}

impl HttpErrorForRetry for HttpDownloadError {
    fn is_retryable(&self) -> bool {
        match self {
            Self::Client(e) => e.is_retryable(),
            Self::InvalidChecksum { .. } => {
                // Normally, invalid checksums don't make sense to retry, but the HTTP servers we
                // talk to internally tend to happily return 200s and give you the error in the
                // message body... so it's a good idea to retry those.
                cfg!(fbcode_build)
            }
            Self::IoError(..) | Self::MaybeNotAllowedOnVpnless { .. } => false,
        }
    }
}

impl IntoBuck2Error for HttpHeadError {
    fn into_buck2_error(self) -> buck2_error::Error {
        buck2_error::Error::from(self)
    }
}

impl IntoBuck2Error for HttpDownloadError {
    fn into_buck2_error(self) -> buck2_error::Error {
        buck2_error::Error::from(self)
    }
}

pub async fn http_head(client: &HttpClient, url: &str) -> buck2_error::Result<Response<()>> {
    let response = http_retry(
        || async {
            client
                .head(url)
                .await
                .map_err(|e| HttpHeadError::Client(HttpError::Client(e)))
        },
        vec![2, 4, 8].into_iter().map(Duration::from_secs).collect(),
    )
    .await?;
    Ok(response)
}

pub async fn http_download(
    client: &HttpClient,
    fs: &ProjectRoot,
    digest_config: DigestConfig,
    path: &ProjectRelativePath,
    url: &str,
    checksum: &Checksum,
    executable: bool,
) -> buck2_error::Result<TrackedFileDigest> {
    let abs_path = fs.resolve(path);
    if let Some(dir) = abs_path.parent() {
        fs_util::create_dir_all(dir)?;
    }

    Ok(http_retry(
        || async {
            let response = client
                .get(url)
                .await
                .map_err(|e| HttpDownloadError::Client(HttpError::Client(e)))?;

            let (head, stream) = response.into_parts();
            let file = fs_util::create_file(&abs_path)
                .categorize_internal()
                .map_err(HttpDownloadError::IoError)?;
            let buf_writer = std::io::BufWriter::new(file);

            let digest = copy_and_hash(
                url,
                Some(head),
                &abs_path,
                stream,
                buf_writer,
                digest_config.cas_digest_config(),
                checksum,
                client.supports_vpnless(),
            )
            .await?;

            if executable {
                fs.set_executable(path)
                    .map_err(HttpDownloadError::IoError)?;
            }

            Result::<_, HttpDownloadError>::Ok(TrackedFileDigest::new(
                digest,
                digest_config.cas_digest_config(),
            ))
        },
        vec![2, 4, 8].into_iter().map(Duration::from_secs).collect(),
    )
    .await
    .map_err(|e| e.into_final())?)
}

/// Copy a stream into a writer while producing its digest and checksumming it.
async fn copy_and_hash(
    url: &str,
    head: Option<http::response::Parts>,
    abs_path: &(impl std::fmt::Display + ?Sized),
    mut stream: impl Stream<Item = Result<Bytes, hyper::Error>> + Unpin,
    mut writer: impl Write,
    digest_config: CasDigestConfig,
    checksum: &Checksum,
    is_vpnless: bool,
) -> Result<FileDigest, HttpDownloadError> {
    let mut digester = FileDigest::digester(digest_config);

    // For each checksum entry we have, we're going to add a validator. We might have to create
    // a new hasher, or reuse the `FileDigest::digester` if it matches. All of them are fed in the
    // same pass over the stream.

    enum Validator {
        PrimaryDigest,
        ExtraDigest(DynHasher),
    }

    let primary = digester.algorithm();
    let mut validators = SmallVec::<[_; 3]>::new();

    for (algorithm, expected) in checksum.pairs() {
        let spec = algorithm.spec();
        let validator = if spec.digest_family == Some(primary) {
            Validator::PrimaryDigest
        } else {
            Validator::ExtraDigest((spec.new_hasher)())
        };
        validators.push((validator, expected, spec.name));
    }

    let mut buff = DebugBuffer::new(512);

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|source| HttpError::Transfer {
            received: digester.bytes_read(),
            url: url.to_owned(),
            source,
        })?;

        buff.peek(&chunk);

        writer
            .write_all(&chunk)
            .with_buck_error_context(|| format!("write({abs_path})"))
            .map_err(HttpDownloadError::IoError)?;

        digester.update(&chunk);
        for (validator, _expected, _kind) in validators.iter_mut() {
            if let Validator::ExtraDigest(hasher) = validator {
                hasher.update(&chunk);
            }
        }
    }
    writer
        .flush()
        .with_buck_error_context(|| format!("flush({abs_path})"))
        .map_err(HttpDownloadError::IoError)?;

    let digest = digester.finalize();

    // Validate
    for (validator, expected, kind) in validators {
        let obtained = match validator {
            Validator::PrimaryDigest => digest.raw_digest().to_string(),
            Validator::ExtraDigest(hasher) => hex::encode(hasher.finalize()),
        };

        if expected != obtained {
            let debug = Box::new(MaybeResponseDebugInfo {
                bytes_seen: buff.bytes_seen,
                buff: buff.to_utf8().map(ToOwned::to_owned),
                head,
                is_final: false,
            });

            if is_vpnless {
                return Err(HttpDownloadError::MaybeNotAllowedOnVpnless {
                    kind,
                    want: expected.to_owned(),
                    got: obtained,
                    url: url.to_owned(),
                    debug,
                });
            }
            return Err(HttpDownloadError::InvalidChecksum {
                digest_kind: kind,
                expected: expected.to_owned(),
                obtained,
                url: url.to_owned(),
                debug,
            });
        }
    }

    Ok(digest)
}

struct DebugBuffer {
    bytes_seen: u64,
    max_size: usize,
    buff: Vec<u8>,
}

impl DebugBuffer {
    fn new(max_size: usize) -> Self {
        Self {
            bytes_seen: 0,
            max_size,
            buff: Vec::new(),
        }
    }

    fn peek(&mut self, chunk: &[u8]) {
        // unwrap safety: we can't possibly have a chunk whose length can't be represented with 64
        // bits.
        self.bytes_seen += u64::try_from(chunk.len()).unwrap();

        if self.buff.len() < self.max_size {
            let want_bytes = std::cmp::min(chunk.len(), self.max_size - self.buff.len());
            self.buff.extend(&chunk[..want_bytes]);
        }
    }

    fn to_utf8(&self) -> Option<&str> {
        match std::str::from_utf8(&self.buff) {
            Ok(utf8) => Some(utf8),
            Err(e) => {
                let valid_up_to = e.valid_up_to();

                // If at least 50% of the buffer is valid UTF8, let's show it.
                if valid_up_to >= (self.max_size / 2) {
                    std::str::from_utf8(&self.buff[..valid_up_to]).ok()
                } else {
                    None
                }
            }
        }
    }
}

/// A little helper to avoid showing the debug data in http_retry because that's reall verbose.
#[derive(Debug)]
struct MaybeResponseDebugInfo {
    bytes_seen: u64,
    buff: Option<String>,
    head: Option<http::response::Parts>,
    is_final: bool,
}

impl fmt::Display for MaybeResponseDebugInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Received {} bytes.", self.bytes_seen)?;

        if !self.is_final {
            return Ok(());
        }

        if let Some(head) = &self.head {
            let interesting_headers = ["error-mid", "x-fb-debug"]
                .into_iter()
                .filter_map(|header| {
                    head.headers
                        .get(header)
                        .and_then(|h| h.to_str().ok())
                        .map(|header_value| (header, header_value))
                })
                .collect::<Vec<_>>();

            write!(f, "\n\nRelevant debug headers:\n")?;
            if interesting_headers.is_empty() {
                write!(f, "<none>")?;
            } else {
                for (header, header_value) in interesting_headers {
                    writeln!(f, "{header}: {header_value}")?;
                }
            }
        }

        match &self.buff {
            Some(text) => {
                write!(f, "\n\nResponse started with:\n\n{text}")?;
            }
            None => {
                write!(f, "Response is not UTF-8")?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use buck2_common::cas_digest::testing;
    use futures::stream;

    use super::*;

    async fn do_test(
        digest_config: CasDigestConfig,
        checksum: &Checksum,
    ) -> Result<(FileDigest, Vec<u8>), HttpDownloadError> {
        let mut out = Vec::new();

        let digest = copy_and_hash(
            "test",
            None,
            "test",
            stream::iter(vec![Ok(Bytes::from("foo")), Ok(Bytes::from("bar"))]),
            &mut out,
            digest_config,
            checksum,
            false,
        )
        .await?;

        Ok((digest, out))
    }

    const FOOBAR_SHA1: &str = "8843d7f92416211de9ebb963ff4ce28125932878";
    const FOOBAR_SHA256: &str = "c3ab8ff13720e8ad9047dd39466b3c8974e592c2fa383d4a3960714caef0c4f2";
    const FOOBAR_SHA512: &str = "0a50261ebd1a390fed2bf326f2673c145582a6342d523204973d0219337f81616a8069b012587cf5635f6925f1b56c360230c19b273500ee013e030601bf2425";

    fn all_three() -> Checksum {
        Checksum::from_pairs(&[
            (ChecksumAlgorithm::Sha1, FOOBAR_SHA1),
            (ChecksumAlgorithm::Sha256, FOOBAR_SHA256),
            (ChecksumAlgorithm::Sha512, FOOBAR_SHA512),
        ])
        .unwrap()
    }

    #[tokio::test]
    async fn test_copy_and_hash_ok() -> buck2_error::Result<()> {
        // None of the checksums is the digest config's algorithm, so each has its own hasher.
        let (digest, bytes) = do_test(testing::blake3(), &all_three()).await?;

        assert_eq!(
            digest.to_string(),
            "aa51dcd43d5c6c5203ee16906fd6b35db298b9b2e1de3fce81811d4806b76b7d:6"
        );

        assert_eq!(std::str::from_utf8(&bytes).unwrap(), "foobar");

        Ok(())
    }
    /// `copy_and_hash` writes every byte of a chunk even when the writer accepts a chunk in
    /// several pieces.
    #[tokio::test]
    async fn test_copy_and_hash_short_write() -> buck2_error::Result<()> {
        struct ShortWriter<'a>(&'a mut Vec<u8>);
        impl Write for ShortWriter<'_> {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                let n = buf.len().min(2);
                self.0.extend_from_slice(&buf[..n]);
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let mut out = Vec::new();
        let digest = copy_and_hash(
            "test",
            None,
            "test",
            stream::iter(vec![Ok(Bytes::from("foo")), Ok(Bytes::from("bar"))]),
            ShortWriter(&mut out),
            testing::sha1(),
            &Checksum::from_pairs(&[(ChecksumAlgorithm::Sha1, FOOBAR_SHA1)])?,
            false,
        )
        .await?;

        assert_eq!(digest.size(), 6);
        assert_eq!(std::str::from_utf8(&out).unwrap(), "foobar");
        Ok(())
    }

    #[tokio::test]
    async fn test_copy_and_hash_ok_with_primary_digest() -> buck2_error::Result<()> {
        // One pass produces the digest config's digest and checks all three checksums, whether
        // or not one of them is the digest config's own algorithm.
        let (digest, bytes) = do_test(testing::sha256(), &all_three()).await?;
        assert_eq!(digest.to_string(), format!("{FOOBAR_SHA256}:6"));
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), "foobar");

        let (digest, _) = do_test(testing::sha1(), &all_three()).await?;
        assert_eq!(digest.to_string(), format!("{FOOBAR_SHA1}:6"));

        Ok(())
    }

    #[tokio::test]
    async fn test_copy_and_hash_sha512_only() -> buck2_error::Result<()> {
        // A checksum that is only a SHA-512, as in an npm or yarn lock file, still gets the
        // digest config's digest (and the right size) from the same pass.
        let checksum = Checksum::from_pairs(&[(ChecksumAlgorithm::Sha512, FOOBAR_SHA512)])?;

        let (digest, bytes) = do_test(testing::sha256(), &checksum).await?;
        assert_eq!(digest.to_string(), format!("{FOOBAR_SHA256}:6"));
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), "foobar");

        let (digest, _) = do_test(testing::blake3(), &checksum).await?;
        assert_eq!(digest.size(), 6);

        Ok(())
    }

    #[tokio::test]
    async fn test_copy_and_hash_invalid_sha512() -> buck2_error::Result<()> {
        // A digest of the right length for another input, so a hasher that ignored its input
        // or compared the wrong digest would be caught.
        let wrong = "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e";
        for config in [testing::sha1(), testing::sha256(), testing::blake3()] {
            assert_matches!(
                do_test(
                    config,
                    &Checksum::from_pairs(&[(ChecksumAlgorithm::Sha512, wrong)])?
                )
                .await,
                Err(HttpDownloadError::InvalidChecksum {
                    digest_kind: "sha512",
                    ..
                })
            );
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_copy_and_hash_one_wrong_checksum_among_several() -> buck2_error::Result<()> {
        // Every checksum is enforced, not just the first or the last.
        let wrong_sha512 = "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e";
        for checksum in [
            Checksum::from_pairs(&[
                (ChecksumAlgorithm::Sha1, FOOBAR_SHA1),
                (ChecksumAlgorithm::Sha256, FOOBAR_SHA256),
                (ChecksumAlgorithm::Sha512, wrong_sha512),
            ])?,
            Checksum::from_pairs(&[
                (ChecksumAlgorithm::Sha512, wrong_sha512),
                (ChecksumAlgorithm::Sha1, FOOBAR_SHA1),
                (ChecksumAlgorithm::Sha256, FOOBAR_SHA256),
            ])?,
        ] {
            assert_matches!(
                do_test(testing::blake3(), &checksum).await,
                Err(HttpDownloadError::InvalidChecksum {
                    digest_kind: "sha512",
                    ..
                })
            );
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_copy_and_hash_invalid_primary_hash() -> buck2_error::Result<()> {
        assert_matches!(
            do_test(
                testing::sha1(),
                &Checksum::unvalidated(&[(ChecksumAlgorithm::Sha1, "oops")])
            )
            .await,
            Err(HttpDownloadError::InvalidChecksum { .. })
        );

        assert_matches!(
            do_test(
                testing::sha256(),
                &Checksum::unvalidated(&[(ChecksumAlgorithm::Sha256, "oops")])
            )
            .await,
            Err(HttpDownloadError::InvalidChecksum { .. })
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_copy_and_hash_invalid_secondary_hash() -> buck2_error::Result<()> {
        assert_matches!(
            do_test(
                testing::blake3(),
                &Checksum::unvalidated(&[(ChecksumAlgorithm::Sha1, "oops")])
            )
            .await,
            Err(HttpDownloadError::InvalidChecksum { .. })
        );

        assert_matches!(
            do_test(
                testing::blake3(),
                &Checksum::unvalidated(&[(ChecksumAlgorithm::Sha256, "oops")])
            )
            .await,
            Err(HttpDownloadError::InvalidChecksum { .. })
        );

        Ok(())
    }

    #[test]
    fn test_checksum_from_pairs() -> buck2_error::Result<()> {
        let checksum = all_three();
        assert_eq!(checksum.sha1(), Some(FOOBAR_SHA1));
        assert_eq!(checksum.sha256(), Some(FOOBAR_SHA256));
        assert_eq!(checksum.sha512(), Some(FOOBAR_SHA512));

        let only_512 = Checksum::from_pairs(&[(ChecksumAlgorithm::Sha512, FOOBAR_SHA512)])?;
        assert_eq!(only_512.sha1(), None);
        assert_eq!(only_512.sha256(), None);
        assert_eq!(only_512.sha512(), Some(FOOBAR_SHA512));

        Ok(())
    }

    #[test]
    fn test_checksum_from_pairs_lowercases() -> buck2_error::Result<()> {
        // Hex is accepted in either case and stored in lowercase, like the other algorithms.
        let upper = Checksum::from_pairs(&[(
            ChecksumAlgorithm::Sha512,
            FOOBAR_SHA512.to_ascii_uppercase().as_str(),
        )])?;
        assert_eq!(upper.sha512(), Some(FOOBAR_SHA512));
        Ok(())
    }

    fn from_pairs_error(entries: &[(ChecksumAlgorithm, &str)]) -> String {
        format!("{:#}", Checksum::from_pairs(entries).unwrap_err())
    }

    #[test]
    fn test_checksum_from_pairs_refuses_an_empty_list() {
        assert!(from_pairs_error(&[]).contains("at least one checksum"));
    }

    #[test]
    fn test_checksum_from_pairs_refuses_a_digest_of_the_wrong_length() {
        assert!(
            from_pairs_error(&[(ChecksumAlgorithm::Sha512, FOOBAR_SHA256)])
                .contains("expected length of 128")
        );
        assert!(
            from_pairs_error(&[(ChecksumAlgorithm::Sha512, FOOBAR_SHA1)])
                .contains("expected length of 128")
        );
        assert!(
            from_pairs_error(&[(ChecksumAlgorithm::Sha256, FOOBAR_SHA512)])
                .contains("expected length of 64")
        );
    }

    #[test]
    fn test_checksum_from_pairs_refuses_non_hex_characters() {
        assert!(
            from_pairs_error(&[(ChecksumAlgorithm::Sha512, &FOOBAR_SHA512.replace('0', "z"))])
                .contains("expected 0-9 a-z hex characters")
        );
    }

    #[test]
    fn test_checksum_from_pairs_refuses_an_algorithm_given_twice() {
        assert!(
            from_pairs_error(&[
                (ChecksumAlgorithm::Sha1, FOOBAR_SHA1),
                (ChecksumAlgorithm::Sha1, FOOBAR_SHA1)
            ])
            .contains("more than once")
        );
    }

    const COMBINATIONS: [(Option<&str>, Option<&str>); 3] = [
        (Some(FOOBAR_SHA1), None),
        (None, Some(FOOBAR_SHA256)),
        (Some(FOOBAR_SHA1), Some(FOOBAR_SHA256)),
    ];

    /// The checksum a rule that names only a sha1 and/or a sha256 gets.
    fn sha1_sha256(sha1: Option<&str>, sha256: Option<&str>) -> buck2_error::Result<Checksum> {
        let mut pairs = Vec::new();
        if let Some(sha1) = sha1 {
            pairs.push((ChecksumAlgorithm::Sha1, sha1));
        }
        if let Some(sha256) = sha256 {
            pairs.push((ChecksumAlgorithm::Sha256, sha256));
        }
        Checksum::from_pairs(&pairs)
    }

    #[test]
    fn test_sha1_and_sha256_digests_are_kept() -> buck2_error::Result<()> {
        for (sha1, sha256) in COMBINATIONS {
            let checksum = sha1_sha256(sha1, sha256)?;
            assert_eq!(checksum.sha1(), sha1);
            assert_eq!(checksum.sha256(), sha256);
            assert_eq!(checksum.sha512(), None);
        }
        // Digests are stored lowercase, whatever case they were given in.
        let upper = sha1_sha256(
            Some(&FOOBAR_SHA1.to_ascii_uppercase()),
            Some(&FOOBAR_SHA256.to_ascii_uppercase()),
        )?;
        assert_eq!(upper.sha1(), Some(FOOBAR_SHA1));
        assert_eq!(upper.sha256(), Some(FOOBAR_SHA256));
        Ok(())
    }

    #[test]
    fn test_sha1_and_sha256_error_messages() {
        let err = |sha1: Option<&str>, sha256: Option<&str>| {
            format!("{:#}", sha1_sha256(sha1, sha256).unwrap_err())
        };
        assert!(err(None, None).contains("Must pass in at least one checksum (e.g. `sha1 = ...`)"));
        assert!(err(Some("abc"), None).contains(
            "Invalid digest for `sha1` argument, expected length of 40 but got 3, digest `abc`"
        ));
        assert!(
            err(None, Some(FOOBAR_SHA1)).contains(&format!(
                "Invalid digest for `sha256` argument, expected length of 64 but got 40, digest `{FOOBAR_SHA1}`"
            ))
        );
        let bad = FOOBAR_SHA1.replace('8', "z");
        assert!(
            err(Some(&bad), None).contains(&format!(
                "Invalid digest for `sha1` argument, expected 0-9 a-z hex characters, but got `z`, digest `{bad}`"
            ))
        );
        // With two bad digests, the sha1 one is reported.
        assert!(err(Some("abc"), Some("def")).contains("`sha1` argument"));
    }

    #[tokio::test]
    async fn test_copy_and_hash_sha1_and_sha256_checksums() -> buck2_error::Result<()> {
        // The bytes are accepted when the checksum matches, and refused with the message naming
        // the algorithm, for a primary and for a secondary hasher.
        let wrong_sha1 = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
        let wrong_sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        for config in [testing::sha1(), testing::sha256(), testing::blake3()] {
            for (sha1, sha256) in COMBINATIONS {
                let (_, bytes) = do_test(config, &sha1_sha256(sha1, sha256)?).await?;
                assert_eq!(std::str::from_utf8(&bytes).unwrap(), "foobar");
            }

            for (sha1, sha256, kind, expected, obtained) in [
                (Some(wrong_sha1), None, "sha1", wrong_sha1, FOOBAR_SHA1),
                (
                    None,
                    Some(wrong_sha256),
                    "sha256",
                    wrong_sha256,
                    FOOBAR_SHA256,
                ),
                (
                    Some(FOOBAR_SHA1),
                    Some(wrong_sha256),
                    "sha256",
                    wrong_sha256,
                    FOOBAR_SHA256,
                ),
                (
                    Some(wrong_sha1),
                    Some(FOOBAR_SHA256),
                    "sha1",
                    wrong_sha1,
                    FOOBAR_SHA1,
                ),
            ] {
                let err = do_test(config, &sha1_sha256(sha1, sha256)?)
                    .await
                    .unwrap_err();
                assert_matches!(
                    &err,
                    HttpDownloadError::InvalidChecksum { digest_kind, .. } if *digest_kind == kind
                );
                assert!(
                    err.to_string().starts_with(&format!(
                        "Invalid {kind} digest. Expected {expected}, got {obtained}. URL: test."
                    )),
                    "{err}"
                );
            }
        }
        Ok(())
    }

    // `Checksum` derives `Pagable`: it is part of analysis results, which can be written to disk
    // and read by another process when DICE paging is on.

    #[test]
    fn test_checksum_roundtrips_through_pagable() -> buck2_error::Result<()> {
        use pagable::PagableDeserialize;
        use pagable::PagableSerialize;
        use pagable::testing::TestingDeserializer;
        use pagable::testing::TestingSerializer;

        let checksum = all_three();
        let mut serializer = TestingSerializer::new();
        checksum.pagable_serialize(&mut serializer).unwrap();
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let decoded = Checksum::pagable_deserialize(&mut deserializer).unwrap();
        assert_eq!(
            decoded.pairs().collect::<Vec<_>>(),
            checksum.pairs().collect::<Vec<_>>()
        );
        assert_eq!(decoded.sha512(), Some(FOOBAR_SHA512));
        Ok(())
    }

    #[test]
    fn test_hash_algorithm_table() {
        let mut names = buck2_hash::BuckMutSet::default();
        for algorithm in [
            ChecksumAlgorithm::Sha1,
            ChecksumAlgorithm::Sha256,
            ChecksumAlgorithm::Sha512,
        ] {
            // Each algorithm has exactly one entry.
            assert_eq!(
                HASH_ALGORITHMS
                    .iter()
                    .filter(|a| a.algorithm == algorithm)
                    .count(),
                1,
                "{algorithm:?}"
            );
            let spec = algorithm.spec();
            assert!(names.insert(spec.name), "duplicate {}", spec.name);
            // The recorded length is what the hasher really produces.
            assert_eq!(
                (spec.new_hasher)().output_size(),
                spec.digest_len,
                "{}",
                spec.name
            );
        }
        assert_eq!(HASH_ALGORITHMS.len(), names.len());
    }

    #[test]
    fn test_debug_buffer() {
        let mut buff = DebugBuffer::new(10);
        buff.peek(b"foo");
        assert_eq!(buff.to_utf8(), Some("foo"));

        let mut buff = DebugBuffer::new(2);
        buff.peek(b"foo");
        assert_eq!(buff.to_utf8(), Some("fo"));

        let mut buff = DebugBuffer::new(4);
        buff.peek(b"foo");
        buff.peek(b"foo");
        assert_eq!(buff.to_utf8(), Some("foof"));

        // 75% fine.
        let mut buff = DebugBuffer::new(4);
        buff.peek(b"foo");
        buff.peek(&[0xff]);
        assert_eq!(buff.to_utf8(), Some("foo"));

        // 50% fine.
        let mut buff = DebugBuffer::new(4);
        buff.peek(b"fo");
        buff.peek(&[0xff]);
        assert_eq!(buff.to_utf8(), Some("fo"));

        // 25% fine.
        let mut buff = DebugBuffer::new(4);
        buff.peek(b"f");
        buff.peek(&[0xff]);
        assert_eq!(buff.to_utf8(), None);

        // 0% fine.
        let mut buff = DebugBuffer::new(4);
        buff.peek(&[0xff]);
        assert_eq!(buff.to_utf8(), None);
    }
}
