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
use smallvec::SmallVec;

use crate::digest_config::DigestConfig;
use crate::materialize::atomic_publish;

#[derive(Debug, Clone, Dupe, Allocative, Pagable)]
pub enum Checksum {
    Sha1(Arc<str>),
    Sha256(Arc<str>),
    Both { sha1: Arc<str>, sha256: Arc<str> },
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Input)]
enum DownloadFileError {
    #[error("Must pass in at least one checksum (e.g. `sha1 = ...`)")]
    MissingChecksum,
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
    pub fn new(sha1: Option<&str>, sha256: Option<&str>) -> buck2_error::Result<Self> {
        fn is_hex_digit(x: char) -> bool {
            let x = x.to_ascii_lowercase();
            x.is_ascii_digit() || ('a'..='f').contains(&x)
        }

        fn validate_digest(
            digest: Option<&str>,
            digest_len: usize,
            digest_type: &'static str,
        ) -> buck2_error::Result<Option<Arc<str>>> {
            match digest {
                None => Ok(None),
                Some(digest) => {
                    let expected_len = digest_len * 2;
                    if digest.len() != expected_len {
                        return Err(DownloadFileError::InvalidDigestLength {
                            digest: digest.to_owned(),
                            expected_len,
                            digest_type,
                        }
                        .into());
                    }
                    if let Some(bad_char) = digest.chars().find(|x| !is_hex_digit(*x)) {
                        return Err(DownloadFileError::InvalidDigestCharacter {
                            digest: digest.to_owned(),
                            bad_char,
                            digest_type,
                        }
                        .into());
                    }
                    Ok(Some(Arc::from(digest.to_ascii_lowercase())))
                }
            }
        }

        match (
            validate_digest(sha1, SHA1_SIZE, "sha1")?,
            validate_digest(sha256, SHA256_SIZE, "sha256")?,
        ) {
            (Some(sha1), None) => Ok(Checksum::Sha1(sha1)),
            (None, Some(sha256)) => Ok(Checksum::Sha256(sha256)),
            (Some(sha1), Some(sha256)) => Ok(Checksum::Both { sha1, sha256 }),
            (None, None) => Err(DownloadFileError::MissingChecksum.into()),
        }
    }

    pub fn sha1(&self) -> Option<&str> {
        match self {
            Self::Sha1(sha1) => Some(sha1),
            Self::Sha256(..) => None,
            Self::Both { sha1, .. } => Some(sha1),
        }
    }

    pub fn sha256(&self) -> Option<&str> {
        match self {
            Self::Sha1(..) => None,
            Self::Sha256(sha256) => Some(sha256),
            Self::Both { sha256, .. } => Some(sha256),
        }
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
            // Whatever is already at `path`, such as another configuration's download of the same
            // content-based path, stays readable until the complete and verified download
            // replaces it.
            let temp =
                atomic_publish::temp_sibling(&abs_path).map_err(HttpDownloadError::IoError)?;
            let buf_writer = std::io::BufWriter::new(temp.as_file());

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

            atomic_publish::publish(temp, &abs_path, executable)
                .map_err(HttpDownloadError::IoError)?;

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
    // a new hasher, or reuse the `FileDigest::digester` if it matches.

    enum Validator {
        PrimaryDigest,
        ExtraDigest(Box<dyn DynDigest + Send>),
    }

    let mut validators = SmallVec::<[_; 2]>::new();

    if let Some(sha1) = checksum.sha1() {
        let validator = if digester.algorithm() == DigestAlgorithmFamily::Sha1 {
            Validator::PrimaryDigest
        } else {
            Validator::ExtraDigest(Box::new(Sha1::new()) as _)
        };

        validators.push((validator, sha1, "sha1"));
    }

    if let Some(sha256) = checksum.sha256() {
        let validator = if digester.algorithm() == DigestAlgorithmFamily::Sha256 {
            Validator::PrimaryDigest
        } else {
            Validator::ExtraDigest(Box::new(Sha256::new()) as _)
        };

        validators.push((validator, sha256, "sha256"));
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
    use buck2_fs::error::IoResultExt;
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

    #[tokio::test]
    async fn test_copy_and_hash_ok() -> buck2_error::Result<()> {
        let (digest, bytes) = do_test(
            testing::blake3(),
            &Checksum::Both {
                sha1: Arc::from("8843d7f92416211de9ebb963ff4ce28125932878"),
                sha256: Arc::from(
                    "c3ab8ff13720e8ad9047dd39466b3c8974e592c2fa383d4a3960714caef0c4f2",
                ),
            },
        )
        .await?;

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
            &Checksum::Sha1(Arc::from("8843d7f92416211de9ebb963ff4ce28125932878")),
            false,
        )
        .await?;

        assert_eq!(digest.size(), 6);
        assert_eq!(std::str::from_utf8(&out).unwrap(), "foobar");
        Ok(())
    }

    #[tokio::test]
    async fn test_copy_and_hash_invalid_primary_hash() -> buck2_error::Result<()> {
        assert_matches!(
            do_test(testing::sha1(), &Checksum::Sha1(Arc::from("oops"))).await,
            Err(HttpDownloadError::InvalidChecksum { .. })
        );

        assert_matches!(
            do_test(testing::sha256(), &Checksum::Sha256(Arc::from("oops"))).await,
            Err(HttpDownloadError::InvalidChecksum { .. })
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_copy_and_hash_invalid_secondary_hash() -> buck2_error::Result<()> {
        assert_matches!(
            do_test(testing::blake3(), &Checksum::Sha1(Arc::from("oops"))).await,
            Err(HttpDownloadError::InvalidChecksum { .. })
        );

        assert_matches!(
            do_test(testing::blake3(), &Checksum::Sha256(Arc::from("oops"))).await,
            Err(HttpDownloadError::InvalidChecksum { .. })
        );

        Ok(())
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

    /// Serves `content` at `/file` for as many requests as a test makes.
    fn serve(content: Vec<u8>) -> httptest::Server {
        use httptest::Expectation;
        use httptest::matchers::request;
        use httptest::responders;

        let server = httptest::Server::run();
        server.expect(
            Expectation::matching(request::method_path("GET", "/file"))
                .times(..)
                .respond_with(responders::status_code(200).body(content)),
        );
        server
    }

    async fn download(
        server: &httptest::Server,
        project: &ProjectRoot,
        path: &ProjectRelativePath,
        content: &[u8],
    ) -> buck2_error::Result<TrackedFileDigest> {
        buck2_certs::certs::maybe_setup_cryptography();
        let client = buck2_http::HttpClientBuilder::https_with_system_roots()
            .await?
            .build();
        let checksum = Checksum::Sha1(hex::encode(Sha1::digest(content)).into());
        http_download(
            &client,
            project,
            DigestConfig::testing_default(),
            path,
            &server.url_str("/file"),
            &checksum,
            false,
        )
        .await
    }

    /// A target analyzed under several configurations downloads one content-based output path
    /// once per configuration, while consumers of the download that finished first already read
    /// it. They must never see the file missing or partially written.
    #[tokio::test]
    async fn test_redownload_never_exposes_a_missing_or_partial_file() -> buck2_error::Result<()> {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        use std::sync::atomic::Ordering;

        use buck2_core::fs::project::ProjectRootTemp;

        // Large enough to arrive in many chunks, so a reader could catch it half written.
        let content: Vec<u8> = (0..4 << 20).map(|i: u32| i as u8).collect();
        let server = serve(content.clone());
        let project = ProjectRootTemp::new()?;
        let path = ProjectRelativePath::new("out/file")?;
        download(&server, project.path(), path, &content).await?;

        let stop = Arc::new(AtomicBool::new(false));
        let reader = std::thread::spawn({
            let stop = stop.dupe();
            let abs = project.path().resolve(path);
            let content = content.clone();
            move || -> Result<usize, String> {
                let mut reads = 0;
                while !stop.load(Ordering::Relaxed) {
                    match std::fs::read(&abs) {
                        Ok(read) if read == content => reads += 1,
                        Ok(read) => return Err(format!("read {} bytes", read.len())),
                        Err(e) => return Err(e.to_string()),
                    }
                }
                Ok(reads)
            }
        });
        for _ in 0..10 {
            download(&server, project.path(), path, &content).await?;
        }
        stop.store(true, Ordering::Relaxed);

        let reads = reader.join().expect("reader panicked");
        assert!(
            matches!(reads, Ok(n) if n > 0),
            "a reader saw the file missing or incomplete: {reads:?}"
        );
        Ok(())
    }

    /// A download that fails its checksum leaves an earlier one in place, and nothing else.
    #[tokio::test]
    async fn test_failed_download_keeps_the_published_file() -> buck2_error::Result<()> {
        use buck2_core::fs::project::ProjectRootTemp;

        let project = ProjectRootTemp::new()?;
        let path = ProjectRelativePath::new("out/file")?;
        download(&serve(b"good".to_vec()), project.path(), path, b"good").await?;

        let error = download(&serve(b"evil".to_vec()), project.path(), path, b"good").await;
        assert!(
            error.is_err(),
            "a download with the wrong content succeeded"
        );

        let abs = project.path().resolve(path);
        assert_eq!(fs_util::read(&abs).categorize_internal()?, b"good");
        let dir = fs_util::read_dir(abs.parent().unwrap()).categorize_internal()?;
        assert_eq!(dir.count(), 1, "the failed download left a file behind");
        Ok(())
    }
}
