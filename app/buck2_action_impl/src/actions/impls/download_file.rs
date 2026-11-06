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
use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_build_api::actions::Action;
use buck2_build_api::actions::ActionExecutionCtx;
use buck2_build_api::actions::UnregisteredAction;
use buck2_build_api::actions::errors::execute_error::ExecuteError;
use buck2_build_api::actions::execute::action_executor::ActionExecutionKind;
use buck2_build_api::actions::execute::action_executor::ActionExecutionMetadata;
use buck2_build_api::actions::execute::action_executor::ActionOutputs;
use buck2_build_api::artifact_groups::ArtifactGroup;
use buck2_build_signals::env::WaitingData;
use buck2_common::cas_digest::RawDigest;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::FileMetadata;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_common::io::trace::TracingIoProvider;
use buck2_core::category::CategoryRef;
use buck2_core::fs::buck_out_path::BuildArtifactPath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_error::BuckErrorContext;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_error::conversion::from_any_with_tag;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::execute::clean_output_paths::CleanOutputPaths;
use buck2_execute::execute::command_executor::ActionExecutionTimingData;
use buck2_execute::materialize::http::Checksum;
use buck2_execute::materialize::http::http_download;
use buck2_execute::materialize::http::http_head;
use buck2_execute::materialize::materializer::CasDownloadInfo;
use buck2_execute::materialize::materializer::DeclareArtifactPayload;
use buck2_execute::materialize::materializer::DeclareMatchOutcome;
use buck2_execute::re::presence::NegativeCache;
use buck2_hash::BuckMutInsertionSet;
use buck2_http::HttpClient;
use dupe::Dupe;
use mini_vec::MiniBoxSlice;
use pagable::Pagable;
use pagable::pagable_typetag;
use starlark::values::OwnedFrozen;
use starlark::values::Value;

use crate::actions::impls::offline;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum DownloadFileActionError {
    #[error("Exactly one output file must be specified for a download file action, got {0}")]
    WrongNumberOfOutputs(usize),
    #[error(
        "Downloads using content-based path {0} must supply metadata (usually in the form of a sha1 or sha256; a sha512 alone is not enough)!"
    )]
    ContentBasedPathWithoutMetadata(BuildArtifactPath),
}

/// The size a download must have: `size_bytes` when it was given, else the declared metadata's
/// (from the HEAD response). When `size_bytes` was given the metadata's size is `size_bytes`, so
/// one size is checked and a mismatch is reported once.
fn expected_size(metadata: Option<&FileMetadata>, size_bytes: Option<u64>) -> Option<u64> {
    size_bytes.or_else(|| metadata.map(|metadata| metadata.digest.size()))
}

/// Fails if a size was declared (by `size_bytes` or a HEAD response) and the download has another.
fn check_downloaded_size(expected: Option<u64>, actual: u64) -> buck2_error::Result<()> {
    match expected {
        Some(expected) if expected != actual => Err(buck2_error!(
            ErrorTag::DownloadSizeMismatch,
            "Downloaded size ({}) does not match expected size ({})",
            actual,
            expected,
        )),
        _ => Ok(()),
    }
}

enum DeclaredMetadata {
    /// The file's digest is known without downloading it.
    Known(FileMetadata),
    /// No checksum the digest config accepts, or no size: only a download can tell.
    Unknown,
    /// The size had to come from a HEAD request, and that failed.
    HeadFailed(buck2_error::Error),
}

#[derive(Debug, Allocative, Pagable)]
pub(crate) struct UnregisteredDownloadFileAction {
    checksum: Checksum,
    size_bytes: Option<u64>,
    url: Arc<str>,
    vpnless_url: Option<Arc<str>>,
    is_executable: bool,
}

impl UnregisteredDownloadFileAction {
    pub(crate) fn new(
        checksum: Checksum,
        size_bytes: Option<u64>,
        url: Arc<str>,
        vpnless_url: Option<Arc<str>>,
        is_executable: bool,
    ) -> Self {
        Self {
            checksum,
            size_bytes,
            url,
            vpnless_url,
            is_executable,
        }
    }
}

impl UnregisteredAction for UnregisteredDownloadFileAction {
    fn register(
        self: Box<Self>,
        outputs: BuckMutInsertionSet<BuildArtifact>,
        _starlark_data: Option<OwnedFrozen<Value<'static>>>,
        _error_handler: Option<OwnedFrozen<Value<'static>>>,
    ) -> buck2_error::Result<Box<dyn Action>> {
        Ok(Box::new(DownloadFileAction::new(outputs, *self)?))
    }
}

#[derive(Debug, Allocative, Pagable)]
struct DownloadFileAction {
    outputs: MiniBoxSlice<BuildArtifact>,
    inner: UnregisteredDownloadFileAction,
}

impl DownloadFileAction {
    fn new(
        outputs: BuckMutInsertionSet<BuildArtifact>,
        inner: UnregisteredDownloadFileAction,
    ) -> buck2_error::Result<Self> {
        if outputs.len() != 1 {
            Err(DownloadFileActionError::WrongNumberOfOutputs(outputs.len()).into())
        } else {
            Ok(Self {
                outputs: outputs.into_iter().collect(),
                inner,
            })
        }
    }

    fn output(&self) -> &BuildArtifact {
        self.outputs
            .iter()
            .next()
            .expect("a single artifact by construction")
    }

    fn url(&self, client: &HttpClient) -> &Arc<str> {
        if client.supports_vpnless() {
            self.inner.vpnless_url.as_ref().unwrap_or(&self.inner.url)
        } else {
            &self.inner.url
        }
    }

    /// Works out what the downloaded file's metadata will be without downloading it.
    async fn declared_metadata(
        &self,
        client: &HttpClient,
        digest_config: DigestConfig,
    ) -> DeclaredMetadata {
        let digest = if digest_config.cas_digest_config().allows_sha1() {
            self.inner
                .checksum
                .sha1()
                .and_then(|sha1| RawDigest::parse_sha1(sha1.as_bytes()).ok())
        } else if digest_config.cas_digest_config().allows_sha256() {
            self.inner
                .checksum
                .sha256()
                .and_then(|sha256| RawDigest::parse_sha256(sha256.as_bytes()).ok())
        } else {
            None
        };

        let digest = match digest {
            Some(digest) => digest,
            None => return DeclaredMetadata::Unknown,
        };

        let size = match self.inner.size_bytes {
            Some(s) => Some(s),
            None => match self.head_content_length(client).await {
                Ok(size) => size,
                Err(e) => return DeclaredMetadata::HeadFailed(e),
            },
        };

        match size {
            Some(size) => {
                let digest = TrackedFileDigest::new(
                    FileDigest::new(digest, size),
                    digest_config.cas_digest_config(),
                );
                DeclaredMetadata::Known(FileMetadata {
                    digest,
                    is_executable: self.inner.is_executable,
                })
            }
            None => DeclaredMetadata::Unknown,
        }
    }

    /// The `Content-Length` a HEAD request reports for the URL, if the server sends one.
    async fn head_content_length(&self, client: &HttpClient) -> buck2_error::Result<Option<u64>> {
        let url = self.url(client);
        let head = http_head(client, url)
            .await
            .map_err(|e| e.tag([ErrorTag::DownloadFileHeadRequest]))?;

        head.headers()
            .get(http::header::CONTENT_LENGTH)
            .map(|content_length| {
                let content_length = content_length
                    .to_str()
                    .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Http))
                    .buck_error_context("Header is not valid utf-8")?;
                let content_length_number =
                    content_length.parse().with_buck_error_context(|| {
                        format!("Header is not a number: `{content_length}`")
                    })?;
                buck2_error::Ok(content_length_number)
            })
            .transpose()
            .with_buck_error_context(|| {
                format!(
                    "Request to `{}` returned an invalid `{}` header",
                    url,
                    http::header::CONTENT_LENGTH
                )
            })
    }

    fn output_path(
        &self,
        ctx: &dyn ActionExecutionCtx,
        value: &ArtifactValue,
    ) -> buck2_error::Result<ProjectRelativePathBuf> {
        let path = self.output().get_path();
        ctx.fs().resolve_build(
            path,
            path.is_content_based_path()
                .then(|| value.content_based_path_hash())
                .as_ref(),
        )
    }

    /// Declares the output as a CAS download when the CAS already holds the content, which
    /// leaves nothing to fetch from the origin server or to write to disk now. `None` when the
    /// content is not there, is about to expire, or there is no CAS to ask. Asked under
    /// `--local-only` and `--no-remote-cache` too, unlike a write's or copy's probe
    /// (`ActionExecutionCtx::may_probe_cas`): the CAS stands in for the URL here, not for a cache.
    async fn declare_from_cas(
        &self,
        ctx: &dyn ActionExecutionCtx,
        metadata: &FileMetadata,
    ) -> buck2_error::Result<Option<ArtifactValue>> {
        if !ctx.cas_configured() {
            return Ok(None);
        }
        let use_case = ctx.invocation_re_use_case();
        let info = Arc::new(CasDownloadInfo::new_probed(use_case));
        let digest = metadata.digest.data();

        // The CAS reports every digest of size 0 as present, so the probe cannot tell a checksum
        // declared with a size of 0 apart from the empty file. Only the download can.
        if digest.size() == 0
            && !ctx
                .digest_config()
                .cas_digest_config()
                .is_empty_digest(digest)
        {
            return Ok(None);
        }

        // A stale miss only costs the download that would otherwise have been skipped.
        let expiration = match ctx
            .re_client()
            .with_use_case(use_case)
            .check_presence(vec![metadata.digest.dupe()], NegativeCache::Allowed, &info)
            .await
        {
            Ok(presence) => match presence.into_iter().next().flatten() {
                Some(expiration) => expiration,
                None => return Ok(None),
            },
            Err(e) => {
                // Falling back to the download is correct for this action, but a CAS that cannot
                // answer this query would silently turn every download into an origin fetch,
                // which should be visible somewhere.
                buck2_core::soft_error!(
                    "download_file_cas_probe_failed",
                    e.context(format!(
                        "CAS lookup for `{digest}` failed; downloading instead"
                    ))
                );
                return Ok(None);
            }
        };

        let value = ArtifactValue::file(FileMetadata {
            digest: TrackedFileDigest::new_expires(
                digest.dupe(),
                expiration,
                ctx.digest_config().cas_digest_config(),
            ),
            is_executable: metadata.is_executable,
        });
        let path = self.output_path(ctx, &value)?;
        ctx.materializer()
            .declare_cas_many(
                info,
                vec![DeclareArtifactPayload {
                    path,
                    artifact: value.dupe(),
                }],
            )
            .await?;

        Ok(Some(value))
    }

    /// Puts the content at the output path now, unless the materializer already has exactly this
    /// content there from an earlier daemon.
    async fn download(
        &self,
        ctx: &dyn ActionExecutionCtx,
        client: &HttpClient,
        url: &str,
        metadata: Option<&FileMetadata>,
    ) -> buck2_error::Result<(ArtifactValue, ActionExecutionKind)> {
        let materializer = ctx.materializer();

        let path = match metadata {
            Some(metadata) => {
                let value = ArtifactValue::file(metadata.dupe());
                let path = self.output_path(ctx, &value)?;
                if let DeclareMatchOutcome::Match = materializer
                    .declare_match(vec![(path.clone(), value.dupe())])
                    .await?
                {
                    return Ok((value, ActionExecutionKind::Simple));
                }
                path
            }
            None => ctx.fs().resolve_build(self.output().get_path(), None)?,
        };

        // Whatever is at the path is stale or untracked; the `declare_existing` below replaces
        // the materializer's record of it, so the disk has to be cleared to match. The lease is
        // held until that report is in.
        let output_lease = materializer.prepare_outputs(vec![path.clone()]).await?;
        ctx.blocking_executor()
            .execute_io(
                Box::new(CleanOutputPaths {
                    paths: vec![path.clone()],
                }),
                ctx.cancellation_context(),
            )
            .await?;

        let digest = http_download(
            client,
            ctx.fs().fs(),
            ctx.digest_config(),
            &path,
            url,
            &self.inner.checksum,
            self.inner.is_executable,
        )
        .await?;

        // RE knows this file by the digest (checksum, size), where the size came from
        // `size_bytes` or the HEAD response rather than from the content. A wrong size would leave
        // every remote consumer unable to find its input, so it is caught here. `size_bytes` is
        // checked whenever it is given, also when no digest could be declared up front (a
        // sha512-only download, or a checksum the digest config does not accept).
        check_downloaded_size(
            expected_size(metadata, self.inner.size_bytes),
            digest.size(),
        )?;

        // The declared metadata, when there is any, is the value: the checksum verified the
        // content and the size was checked above, and a content-based output path was resolved
        // from it. The digest the download computed is under the digest config's preferred
        // algorithm, which need not be the checksum's, so it only stands in when nothing was
        // declared.
        let value = match metadata {
            Some(metadata) => ArtifactValue::file(metadata.dupe()),
            None => ArtifactValue::file(FileMetadata {
                digest,
                is_executable: self.inner.is_executable,
            }),
        };
        materializer
            .declare_existing(
                &output_lease,
                vec![DeclareArtifactPayload {
                    path,
                    artifact: value.dupe(),
                }],
            )
            .await?;

        Ok((value, ActionExecutionKind::Simple))
    }

    /// Execute this action for offline builds (e.g. no network).
    async fn execute_for_offline(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
    ) -> buck2_error::Result<(ActionOutputs, ActionExecutionMetadata)> {
        let outputs = offline::declare_copy_from_offline_cache(ctx, &[self.output()]).await?;

        Ok((
            outputs,
            ActionExecutionMetadata {
                dep_file_db_writes_queued: 0,
                execution_kind: ActionExecutionKind::Simple,
                timing: ActionExecutionTimingData::default(),
                input_files_bytes: None,
                waiting_data: WaitingData::new(),
            },
        ))
    }
}

#[pagable_typetag]
#[async_trait]
impl Action for DownloadFileAction {
    fn kind(&self) -> buck2_data::ActionKind {
        buck2_data::ActionKind::DownloadFile
    }

    fn inputs(&self) -> buck2_error::Result<Cow<'_, [ArtifactGroup]>> {
        Ok(Cow::Borrowed(&[]))
    }

    fn outputs(&self) -> Cow<'_, [BuildArtifact]> {
        Cow::Borrowed(&self.outputs)
    }

    fn first_output(&self) -> &BuildArtifact {
        self.output()
    }

    fn category(&self) -> CategoryRef<'_> {
        CategoryRef::unchecked_new("download_file")
    }

    fn identifier(&self) -> Option<&str> {
        self.outputs
            .iter()
            .next()
            .map(|o| o.get_path().path().as_str())
    }

    async fn execute(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
        waiting_data: WaitingData,
    ) -> Result<(ActionOutputs, ActionExecutionMetadata), ExecuteError> {
        // Early return - if this path exists, it's because we're running in a
        // special offline mode where the HEAD request below will likely fail.
        // Shortcut and just return this path as the action output.
        //
        // This mostly looks like a "copy" action.
        if ctx.run_action_knobs().use_network_action_output_cache {
            return self.execute_for_offline(ctx).await.map_err(Into::into);
        }

        let client = ctx.http_client();
        let url = self.url(client);
        let is_content_based = self.output().get_path().is_content_based_path();

        let metadata = match self.declared_metadata(client, ctx.digest_config()).await {
            DeclaredMetadata::Known(metadata) => Some(metadata),
            DeclaredMetadata::Unknown => None,
            DeclaredMetadata::HeadFailed(e) => {
                if is_content_based {
                    return Err(e.into());
                }
                // Not a soft error: servers legitimately refuse HEAD, and a server that is
                // actually down fails the GET that follows.
                tracing::debug!(
                    "HEAD request for `{}` failed, downloading instead: {:#}",
                    url,
                    e
                );
                None
            }
        };
        if metadata.is_none() && is_content_based {
            return Err(ExecuteError::Error {
                error: DownloadFileActionError::ContentBasedPathWithoutMetadata(
                    self.output().get_path().dupe(),
                )
                .into(),
            });
        }

        let (value, execution_kind) = match &metadata {
            Some(metadata) => match self.declare_from_cas(ctx, metadata).await? {
                Some(value) => (value, ActionExecutionKind::Deferred),
                None => self.download(ctx, client, url, Some(metadata)).await?,
            },
            None => self.download(ctx, client, url, None).await?,
        };

        // If we're tracing I/O, get the materializer to copy to the offline cache
        // so we can include it in the offline archive manifest later.
        let io_provider = ctx.io_provider();
        if let Some(tracer) = TracingIoProvider::from_io(io_provider) {
            offline::declare_copy_to_offline_output_cache(ctx, tracer, self.output(), value.dupe())
                .await?;
        }

        Ok((
            ActionOutputs::from_single(self.output().get_path().dupe(), value),
            ActionExecutionMetadata {
                dep_file_db_writes_queued: 0,
                execution_kind,
                timing: ActionExecutionTimingData::default(),
                input_files_bytes: None,
                waiting_data,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use buck2_common::cas_digest::testing;

    use super::*;

    // TODO: This needs proper tests, but right now it's kind of a pain to get the
    //       action framework up and running to test actions
    #[test]
    fn downloads_file() {}

    #[test]
    fn test_check_downloaded_size() {
        // A wrong size is refused, also when the checksum alone cannot name a CAS digest (a
        // sha512-only download).
        let err = check_downloaded_size(Some(6), 7).unwrap_err();
        assert!(err.has_tag(ErrorTag::DownloadSizeMismatch));
        assert!(
            format!("{err:#}").contains("Downloaded size (7) does not match expected size (6)")
        );

        // A right size, or no declared size, is accepted.
        check_downloaded_size(Some(6), 6).unwrap();
        check_downloaded_size(None, 6).unwrap();
    }

    #[test]
    fn content_based_path_error_says_sha512_is_not_enough() {
        use buck2_core::configuration::data::ConfigurationData;
        use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
        use buck2_core::fs::buck_out_path::BuckOutPathKind;
        use buck2_core::target::label::label::TargetLabel;
        use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;

        let target =
            TargetLabel::testing_parse("cell//pkg:t").configure(ConfigurationData::testing_new());
        let err = DownloadFileActionError::ContentBasedPathWithoutMetadata(BuildArtifactPath::new(
            BaseDeferredKey::TargetLabel(target),
            ForwardRelativePathBuf::unchecked_new("out".into()),
            BuckOutPathKind::Configuration,
        ));
        let msg = format!("{err}");
        assert!(msg.contains("a sha1 or sha256"), "{msg}");
        assert!(msg.contains("a sha512 alone is not enough"), "{msg}");
    }

    #[test]
    fn test_expected_size() {
        // `size_bytes` is checked whenever it is given, with or without declared metadata (none
        // is declared for a sha512-only download, or a sha256 under a sha1 digest config).
        assert_eq!(expected_size(None, Some(6)), Some(6));
        assert_eq!(expected_size(None, None), None);

        let metadata = FileMetadata {
            digest: TrackedFileDigest::new(
                FileDigest::from_content(b"foobar", testing::sha256()),
                testing::sha256(),
            ),
            is_executable: false,
        };
        // Metadata carries `size_bytes` when that was given; alone it is the HEAD response's size.
        assert_eq!(expected_size(Some(&metadata), Some(6)), Some(6));
        assert_eq!(expected_size(Some(&metadata), None), Some(6));
    }
}
