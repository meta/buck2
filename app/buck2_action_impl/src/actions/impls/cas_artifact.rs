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
use std::slice;
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
use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::FileMetadata;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_common::io::trace::TracingIoProvider;
use buck2_core::category::CategoryRef;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_core::soft_error;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::ErrorTag;
use buck2_error::internal_error;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::digest::CasDigestToReExt;
use buck2_execute::directory::ActionDirectoryEntry;
use buck2_execute::directory::INTERNER;
use buck2_execute::directory::re_directory_to_re_tree;
use buck2_execute::directory::re_tree_to_directory;
use buck2_execute::execute::command_executor::ActionExecutionTimingData;
use buck2_execute::materialize::materializer::CasDownloadInfo;
use buck2_execute::materialize::materializer::DeclareArtifactPayload;
use buck2_execute::re::manager::ManagedRemoteExecutionClient;
use buck2_execute::re::presence::NegativeCache;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_hash::BuckMutInsertionSet;
use dupe::Dupe;
use jiff::SignedDuration;
use jiff::Timestamp;
use pagable::Pagable;
use pagable::pagable_typetag;
use remote_execution as RE;
use starlark::values::OwnedFrozen;
use starlark::values::Value;

use crate::actions::impls::offline;

#[derive(Debug, buck2_error::Error)]
enum CasArtifactActionDeclarationError {
    #[error("CAS artifact action should have exactly 1 output, got {0}")]
    #[buck2(tag = ReCasArtifactWrongNumberOfOutputs)]
    WrongNumberOfOutputs(usize),
}

#[derive(Debug, buck2_error::Error)]
enum CasArtifactActionExecutionError {
    #[error(
        "The digest `{digest}` was declared to expire after `{declared_expiration}`, but it was set to expire at `{effective_expiration}`{}"
    , (if .effective_expiration != .updated_expiration {format!(" (updated to {})", .updated_expiration)} else {"".to_owned()}))]
    #[buck2(tag = ReCasArtifactInvalidExpiration)]
    InvalidExpiration {
        digest: FileDigest,
        declared_expiration: Timestamp,
        effective_expiration: Timestamp,
        updated_expiration: Timestamp,
    },
    #[error("The digest `{digest}` was not found in the CAS under use case `{use_case}`")]
    #[buck2(tag = DeclaredArtifactNotFound)]
    NotFound {
        digest: FileDigest,
        use_case: RemoteExecutorUseCase,
    },
}

#[derive(Debug, Allocative, Clone, Dupe, Copy, Pagable)]
pub(crate) enum DirectoryKind {
    Directory,
    Tree,
}

#[derive(Debug, Allocative, Pagable)]
pub(crate) enum ArtifactKind {
    Directory(DirectoryKind),
    File,
}

/// How an expiration is looked up; see `CasArtifactAction::expiration_of`.
#[derive(Clone, Copy)]
enum Lookup {
    Shared,
    Direct,
}

/// Where `reconcile_into` decided a file artifact is served from, and what the lookups that made
/// the decision learned on the way, so the main flow does not ask RE a second time.
struct Served {
    use_case: RemoteExecutorUseCase,
    /// The blob's expiration in the canonical namespace, if served from there and known. This is
    /// the only expiration the declared value may record: the digests the daemon shares describe
    /// that namespace and no other.
    canonical_expiration: Option<Timestamp>,
    /// The blob's expiration under the action's own use case, if a lookup there was needed.
    own_expiration: Option<Timestamp>,
}

/// This is an action that lets you reference a CAS artifact. Notionally it's a bit like
/// download_file. When the action executes it'll just verify that the content exists. You have to
/// provide an minimum expiration timestamp when you add this to force users to think about the TTL
/// of the artifacts they are referencing (though admittedly this was also an issue in
/// download_file).
#[derive(Debug, Allocative, Pagable)]
pub(crate) struct UnregisteredCasArtifactAction {
    pub(crate) digest: FileDigest,
    pub(crate) re_use_case: RemoteExecutorUseCase,
    /// We require the caller to declare when this digest will expire. The intention is to force
    /// callers to pay some modicum of attention to when their digests expire.
    #[allocative(skip)]
    #[pagable(flatten_serde)]
    pub(crate) expires_after: Timestamp,
    pub(crate) executable: bool,
    pub(crate) kind: ArtifactKind,
}

impl UnregisteredAction for UnregisteredCasArtifactAction {
    fn register(
        self: Box<Self>,
        outputs: BuckMutInsertionSet<BuildArtifact>,
        _starlark_data: Option<OwnedFrozen<Value<'static>>>,
        _error_handler: Option<OwnedFrozen<Value<'static>>>,
    ) -> buck2_error::Result<Box<dyn Action>> {
        Ok(Box::new(CasArtifactAction::new(outputs, *self)?))
    }
}

#[derive(Debug, Allocative, Pagable)]
struct CasArtifactAction {
    output: BuildArtifact,
    inner: UnregisteredCasArtifactAction,
}

impl CasArtifactAction {
    fn new(
        outputs: BuckMutInsertionSet<BuildArtifact>,
        inner: UnregisteredCasArtifactAction,
    ) -> buck2_error::Result<Self> {
        let outputs_len = outputs.len();
        let mut outputs = outputs.into_iter();

        let output = match (outputs.next(), outputs.next()) {
            (Some(output), None) => output,
            _ => {
                return Err(
                    CasArtifactActionDeclarationError::WrongNumberOfOutputs(outputs_len).into(),
                );
            }
        };

        Ok(Self { output, inner })
    }

    /// When the CAS behind `re_client` will drop the digest, or `None` when it does not have it.
    /// The read is authoritative: a stale miss would fail the build.
    ///
    /// A `Shared` lookup goes through the presence check, which answers from an expiration the
    /// digest already carries and records what RE says on it. The expirations digests carry
    /// describe the one CAS namespace the daemon works in, so `Shared` is only correct for a
    /// client in that namespace. A `Direct` lookup asks RE and records nothing: what a foreign
    /// namespace needs, and also what a decision needs when the digest may carry a foreign
    /// namespace's expiration (see `reconcile_into`); an answer obtained directly in the daemon's
    /// own namespace is still fine for the caller to record.
    async fn expiration_of(
        &self,
        ctx: &dyn ActionExecutionCtx,
        re_client: &ManagedRemoteExecutionClient,
        info: &CasDownloadInfo,
        lookup: Lookup,
    ) -> buck2_error::Result<Option<Timestamp>> {
        let context = || {
            format!(
                "Error accessing digest expiration for: `{}`",
                self.inner.digest,
            )
        };
        match lookup {
            Lookup::Shared => {
                let digest = TrackedFileDigest::new(
                    self.inner.digest.dupe(),
                    ctx.digest_config().cas_digest_config(),
                );
                re_client
                    .check_presence(vec![digest], NegativeCache::Bypassed, info)
                    .await
                    .with_buck_error_context(context)?
                    .into_iter()
                    .next()
                    .internal_error("check_presence did not return anything")
                    .tag(ErrorTag::ReCasArtifactGetDigestExpirationError)
            }
            Lookup::Direct => {
                let now = Timestamp::now();
                let expiration = re_client
                    .get_digest_expirations(vec![self.inner.digest.to_re()], info)
                    .await
                    .with_buck_error_context(context)?
                    .into_iter()
                    .next()
                    .internal_error("get_digest_expirations did not return anything")
                    .tag(ErrorTag::ReCasArtifactGetDigestExpirationError)?
                    .1;
                // RE reports a missing blob as one that expired in the past.
                Ok((expiration > now).then_some(expiration))
            }
        }
    }

    /// The use case a file artifact is served from: `canonical` when the CAS already holds the
    /// blob there or it could be copied there from the action's own use case, otherwise the
    /// action's own.
    ///
    /// Copying is best effort. Failing to reach or write the canonical namespace leaves the
    /// artifact where it was, which is what every consumer got before reconciliation existed.
    async fn reconcile_into(
        &self,
        ctx: &dyn ActionExecutionCtx,
        canonical: RemoteExecutorUseCase,
    ) -> buck2_error::Result<Served> {
        let own = self.inner.re_use_case;
        let canonical_client = ctx.re_client().with_use_case(canonical);
        let canonical_info = CasDownloadInfo::new_declared(canonical);
        // Direct even though the client is in the daemon's namespace: this decision must not
        // read an expiration the digest already carries, because that may have been recorded by
        // something that saw the same content in another namespace (the deferred materializer's
        // refresher stamping the leaves of a foreign-served artifact; in tests, a command run
        // under another use case). The answer itself is the daemon's namespace's truth and is
        // handed back for the caller to record. Once that refresher is gone nothing else can
        // stamp a foreign namespace's expiration, and this check can go through the shared
        // presence layer like every other lookup in the daemon's namespace.
        match self
            .expiration_of(ctx, &canonical_client, &canonical_info, Lookup::Direct)
            .await
        {
            Ok(Some(expiration)) => {
                return Ok(Served {
                    use_case: canonical,
                    canonical_expiration: Some(expiration),
                    own_expiration: None,
                });
            }
            Ok(None) => {}
            Err(e) => {
                soft_error!(
                    "cas_artifact_canonical_check_failed",
                    e.context(format!(
                        "Could not check `{}` under use case `{canonical}`; serving it from `{own}`",
                        self.inner.digest
                    )));
                return Ok(Served {
                    use_case: own,
                    canonical_expiration: None,
                    own_expiration: None,
                });
            }
        }

        // Nothing in the canonical namespace; see whether the action's own has it.
        let own_client = ctx.re_client().with_use_case(own);
        let own_info = CasDownloadInfo::new_declared(own);
        let Some(own_expiration) = self
            .expiration_of(ctx, &own_client, &own_info, Lookup::Direct)
            .await?
        else {
            // Not there either; the caller reports that against the action's own use case.
            return Ok(Served {
                use_case: own,
                canonical_expiration: None,
                own_expiration: None,
            });
        };

        match self
            .copy_into_canonical(ctx, &own_client, &own_info, &canonical_client)
            .await
        {
            Ok(()) => {
                // The copy has whatever TTL a fresh upload gets; the user's `expires_after` is
                // a statement about their own namespace and is enforced there. Its expiration
                // here is only read, so that the declared value can record it.
                let canonical_expiration = self
                    .expiration_of(ctx, &canonical_client, &canonical_info, Lookup::Direct)
                    .await
                    .ok()
                    .flatten();
                Ok(Served {
                    use_case: canonical,
                    canonical_expiration,
                    own_expiration: Some(own_expiration),
                })
            }
            Err(e) => {
                soft_error!(
                    "cas_artifact_reconcile_failed",
                    e.context(format!(
                        "Could not copy `{}` from use case `{own}` into `{canonical}`; serving it from `{own}`",
                        self.inner.digest
                    )));
                Ok(Served {
                    use_case: own,
                    canonical_expiration: None,
                    own_expiration: Some(own_expiration),
                })
            }
        }
    }

    /// Copies the blob from the action's own use case into the canonical one through a file in
    /// the action's scratch directory, so that neither direction holds the blob in memory: what
    /// gets referenced this way is prebuilt tooling, routinely hundreds of megabytes.
    async fn copy_into_canonical(
        &self,
        ctx: &dyn ActionExecutionCtx,
        own_client: &ManagedRemoteExecutionClient,
        own_info: &CasDownloadInfo,
        canonical_client: &ManagedRemoteExecutionClient,
    ) -> buck2_error::Result<()> {
        let re_digest = self.inner.digest.to_re();
        let scratch_rel = ctx
            .fs()
            .buck_out_path_resolver()
            .resolve_scratch(&ctx.target().scratch_path())?;
        // The scratch directory is written to below, so it is claimed like any other output.
        let _scratch_lease = ctx
            .materializer()
            .prepare_outputs(vec![scratch_rel.clone()])
            .await?;
        let scratch_dir = ctx.fs().fs().resolve(&scratch_rel);
        let scratch_file =
            scratch_dir.join(ForwardRelativePath::unchecked_new("cas_artifact_copy"));
        let scratch_name = scratch_file.as_maybe_relativized_str()?.to_owned();

        let copy = async {
            {
                let scratch_dir = scratch_dir.clone();
                let scratch_file = scratch_file.clone();
                ctx.blocking_executor()
                    .execute_io_inline(move || {
                        fs_util::create_dir_all(&scratch_dir)?;
                        fs_util::uncategorized::remove_all(&scratch_file)
                    })
                    .await?;
            }
            own_client
                .materialize_files(
                    vec![RE::NamedDigestWithPermissions {
                        named_digest: RE::NamedDigest {
                            name: scratch_name.clone(),
                            digest: re_digest.clone(),
                            ..Default::default()
                        },
                        is_executable: false,
                        ..Default::default()
                    }],
                    own_info,
                )
                .await?;
            // The upload names the digest and the CAS takes the name on trust, so what went out
            // is whatever the download wrote; hash it before offering it under that name. Hash
            // with the artifact digest's own algorithm, not the config's preferred one: a daemon
            // that prefers BLAKE3-KEYED still accepts sha1 digests from rules, and a digest of
            // another family could never compare equal.
            let algorithm = ctx
                .digest_config()
                .cas_digest_config()
                .algorithm_for_family(self.inner.digest.raw_digest().algorithm())
                .with_internal_error(|| {
                    format!(
                        "The digest config does not accept the algorithm of `{}`",
                        self.inner.digest
                    )
                })?;
            let downloaded = {
                let scratch_file = scratch_file.clone();
                ctx.blocking_executor()
                    .execute_io_inline(move || {
                        let file = fs_util::open_file(&scratch_file).categorize_internal()?;
                        FileDigest::from_reader_for_algorithm(file, algorithm)
                    })
                    .await?
            };
            if downloaded != self.inner.digest {
                return Err(internal_error!(
                    "Downloading `{}` from use case `{}` produced `{}`",
                    self.inner.digest,
                    own_info.re_use_case,
                    downloaded
                ));
            }
            canonical_client
                .upload_files_and_directories(
                    vec![RE::NamedDigest {
                        name: scratch_name.clone(),
                        digest: re_digest.clone(),
                        ..Default::default()
                    }],
                    vec![],
                    vec![],
                    None,
                )
                .await?;
            buck2_error::Ok(())
        };
        let result = copy.await;
        // The scratch file has done its job either way; a failure to remove it is not worth
        // failing the action over, since the scratch sweep collects it.
        let _ignored = ctx
            .blocking_executor()
            .execute_io_inline(move || fs_util::uncategorized::remove_all(&scratch_file))
            .await;
        result
    }

    async fn execute_for_offline(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
    ) -> buck2_error::Result<(ActionOutputs, ActionExecutionMetadata)> {
        let outputs = offline::declare_copy_from_offline_cache(ctx, &[&self.output]).await?;

        Ok((
            outputs,
            ActionExecutionMetadata {
                dep_file_db_writes_queued: 0,
                execution_kind: ActionExecutionKind::Deferred,
                timing: ActionExecutionTimingData::default(),
                input_files_bytes: None,
                waiting_data: WaitingData::new(),
            },
        ))
    }
}

#[pagable_typetag]
#[async_trait]
impl Action for CasArtifactAction {
    fn kind(&self) -> buck2_data::ActionKind {
        buck2_data::ActionKind::CasArtifact
    }

    fn inputs(&self) -> buck2_error::Result<Cow<'_, [ArtifactGroup]>> {
        Ok(Cow::Borrowed(&[]))
    }

    fn outputs(&self) -> Cow<'_, [BuildArtifact]> {
        Cow::Borrowed(slice::from_ref(&self.output))
    }

    fn first_output(&self) -> &BuildArtifact {
        &self.output
    }

    fn category(&self) -> CategoryRef<'_> {
        CategoryRef::unchecked_new("cas_artifact")
    }

    fn identifier(&self) -> Option<&str> {
        Some(self.output.get_path().path().as_str())
    }

    async fn execute(
        &self,
        ctx: &mut dyn ActionExecutionCtx,
        waiting_data: WaitingData,
    ) -> Result<(ActionOutputs, ActionExecutionMetadata), ExecuteError> {
        // If running in offline environment, try to restore from cached outputs.
        if ctx.run_action_knobs().use_network_action_output_cache {
            return self.execute_for_offline(ctx).await.map_err(Into::into);
        }

        // Everything else in buck2 works in one CAS namespace, the one the command's use cases
        // belong to, and a use case from another namespace is invisible to it: RE workers cannot
        // fetch such a blob, which is what forced `local_only` onto consumers of these artifacts.
        // File artifacts are therefore copied into the canonical namespace when they are not
        // already there. Directory and tree artifacts are not: copying one means walking and
        // re-uploading every node, and no user of this action has needed that yet.
        let own = self.inner.re_use_case;
        let canonical = ctx.invocation_re_use_case();
        let served = match self.inner.kind {
            ArtifactKind::File if own != canonical => self.reconcile_into(ctx, canonical).await?,
            ArtifactKind::File | ArtifactKind::Directory(_) => Served {
                use_case: own,
                canonical_expiration: None,
                own_expiration: None,
            },
        };

        // `expires_after` is the user's statement about the namespace they declared the blob in,
        // so it is checked there, and extended there when short, whichever namespace the blob
        // ends up served from. A lookup in a foreign namespace must leave no trace on the digests
        // the daemon shares, hence direct.
        let own_lookup = if own == canonical {
            Lookup::Shared
        } else {
            Lookup::Direct
        };
        let own_client = ctx.re_client().with_use_case(own);
        let own_info = CasDownloadInfo::new_declared(own);

        // Shared reborrows, so that the closure can be called more than once.
        let ctx_ref: &dyn ActionExecutionCtx = ctx;
        let (own_client_ref, own_info_ref) = (&own_client, &own_info);
        let get_expiration = |lookup: Lookup| async move {
            self.expiration_of(ctx_ref, own_client_ref, own_info_ref, lookup)
                .await?
                .ok_or_else(|| {
                    buck2_error::Error::from(CasArtifactActionExecutionError::NotFound {
                        digest: self.inner.digest.dupe(),
                        use_case: own,
                    })
                })
        };

        let mut expiration = match served.own_expiration {
            Some(expiration) => expiration,
            None => get_expiration(own_lookup).await?,
        };

        if expiration < self.inner.expires_after {
            // The expires_after mechanism is intended to support users storing prebuilt artifacts in cas and asserting that their builds will continue
            // working for some minimum time period (typically years).
            //
            // If the observed expiration is too short, we'll log a soft error and try to extend it.
            //
            // TODO(cjhopman): It would be reasonable for this behavior to be more configurable, there just hasn't been need for it yet.
            let now = Timestamp::now();

            // Adds a small buffer to avoid minor clock skew issues.
            let new_ttl =
                self.inner.expires_after.duration_since(now) + SignedDuration::from_mins(5);

            own_client
                .extend_digest_ttl(
                    vec![self.inner.digest.to_re()],
                    std::time::Duration::try_from(new_ttl)
                        .map_err(|e| internal_error!("casting ttl to std duration `{}`", e))?,
                    &own_info,
                )
                .await?;

            // We were able to extend the ttl, so this won't be failing builds, but we need to report it so we can track it.
            // The digest still carries the expiration from before the extension.
            let new_expiration = get_expiration(Lookup::Direct).await?;
            let error: buck2_error::Error = CasArtifactActionExecutionError::InvalidExpiration {
                digest: self.inner.digest.dupe(),
                declared_expiration: self.inner.expires_after,
                effective_expiration: expiration,
                updated_expiration: new_expiration,
            }
            .into();
            soft_error!("cas_artifact_invalid_expiration", error);
            expiration = new_expiration;
        }

        // What the declared value records is the canonical namespace's expiration, the one the
        // daemon's digests describe: the action's own when that is the canonical use case, the
        // reconciled copy's when it is not, and nothing when the blob is served from elsewhere.
        let recorded = if own == canonical {
            Some(expiration)
        } else {
            served.canonical_expiration
        };
        let re_client = ctx.re_client().with_use_case(served.use_case);
        let cas_download_info = Arc::new(CasDownloadInfo::new_declared(served.use_case));

        let value = match self.inner.kind {
            ArtifactKind::Directory(directory_kind) => {
                // TODO: should honor the semaphore here from OutputTreesDownloadConfig.

                let tree = match directory_kind {
                    DirectoryKind::Tree => re_client
                        .download_typed_blobs::<RE::Tree>(
                            None,
                            vec![self.inner.digest.to_re()],
                            cas_download_info.as_ref(),
                        )
                        .await
                        .and_then(|trees| {
                            trees
                                .into_iter()
                                .next()
                                .internal_error("RE response was empty")
                        })
                        .with_buck_error_context(|| {
                            format!("Error downloading tree: {}", self.inner.digest)
                        })?,
                    DirectoryKind::Directory => {
                        let root_directory = re_client
                            .download_typed_blobs::<RE::Directory>(
                                None,
                                vec![self.inner.digest.to_re()],
                                cas_download_info.as_ref(),
                            )
                            .await
                            .and_then(|dirs| {
                                dirs.into_iter()
                                    .next()
                                    .internal_error("RE response was empty")
                            })
                            .with_buck_error_context(|| {
                                format!("Error downloading dir: {}", self.inner.digest)
                            })?;
                        re_directory_to_re_tree(
                            root_directory,
                            &re_client,
                            cas_download_info.as_ref(),
                        )
                        .await?
                    }
                };

                // NOTE: We assign a zero timestamp here because we didn't check the nodes in the tree,
                // just the tree itself. Perhaps we should, but some of the prospective users for this
                // have very large trees so that might not be wise.
                let dir = re_tree_to_directory(
                    &tree,
                    &Timestamp::UNIX_EPOCH,
                    ctx.digest_config(),
                    ctx.output_trees_download_config()
                        .fingerprint_re_output_trees_eagerly(),
                )
                .buck_error_context("Invalid directory")?;

                ArtifactValue::new(
                    ActionDirectoryEntry::Dir(
                        dir.fingerprint(ctx.digest_config().as_directory_serializer())
                            .shared(&*INTERNER),
                    ),
                    None,
                )
            }
            ArtifactKind::File => {
                let config = ctx.digest_config().cas_digest_config();
                let digest = match recorded {
                    Some(expiration) => {
                        TrackedFileDigest::new_expires(self.inner.digest.dupe(), expiration, config)
                    }
                    None => TrackedFileDigest::new(self.inner.digest.dupe(), config),
                };
                let metadata = FileMetadata {
                    digest,
                    is_executable: self.inner.executable,
                };
                ArtifactValue::file(metadata)
            }
        };

        let path = ctx.fs().resolve_build(
            self.output.get_path(),
            if self.output.get_path().is_content_based_path() {
                Some(value.content_based_path_hash())
            } else {
                None
            }
            .as_ref(),
        )?;
        ctx.materializer()
            .declare_cas_many(
                cas_download_info,
                vec![DeclareArtifactPayload {
                    path,
                    artifact: value.dupe(),
                }],
            )
            .await?;

        let io_provider = ctx.io_provider();
        if let Some(tracer) = TracingIoProvider::from_io(io_provider) {
            offline::declare_copy_to_offline_output_cache(ctx, tracer, &self.output, value.dupe())
                .await?;
        }

        Ok((
            ActionOutputs::from_single(self.output.get_path().dupe(), value),
            ActionExecutionMetadata {
                dep_file_db_writes_queued: 0,
                execution_kind: ActionExecutionKind::Deferred,
                timing: ActionExecutionTimingData::default(),
                input_files_bytes: None,
                waiting_data,
            },
        ))
    }
}
