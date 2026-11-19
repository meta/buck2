/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Write actions served from the action cache (`buck2.write_action_cache`).
//!
//! A write's content is a function of its command line, so a strong hash of that command line
//! (`CommandLineBuilder::new_hashing`) keys an action cache entry whose one output is the
//! content's digest. A hit declares the output as CAS content without rendering the command
//! line. A miss renders it, and where the write is one to record (`WriteCache::use_for`)
//! uploads the content and records the entry; otherwise it is produced the way a run action's
//! output is, locally or remotely (`declare_miss`). `verify` renders every write and compares a
//! hit's content against it, for trying the key out.
//!
//! A write with `absolute` renders the project root, which is not the same between hosts, so
//! it does not go through the cache at all (`WriteAction::execute`).

use std::hash::Hasher;
use std::sync::Arc;

use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_build_api::actions::ActionExecutionCtx;
use buck2_build_api::actions::execute::action_executor::ActionExecutionKind;
use buck2_build_api::actions::impls::run_action_knobs::WriteActionCacheMode;
use buck2_build_api::actions::impls::write_action_cache_stats::WRITE_ACTION_CACHE_STATS;
use buck2_build_api::actions::impls::write_action_cache_stats::WriteActionCacheStats;
use buck2_build_api::interpreter::rule_defs::cmd_args::CommandLineBuilder;
use buck2_build_api::interpreter::rule_defs::cmd_args::value_as::ValueAsCommandLineLike;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::FileMetadata;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::content_hash::ContentBasedPathHash;
use buck2_core::deferred::key::DeferredHolderKey;
use buck2_core::execution_types::executor_config::PathSeparatorKind;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::rollout_percentage::RolloutFraction;
use buck2_core::soft_error_or_panic;
use buck2_error::BuckErrorContext;
use buck2_error::buck2_error;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::digest::CasDigestFromReExt;
use buck2_execute::digest::CasDigestToReExt;
use buck2_execute::execute::action_digest_and_blobs::ActionDigestAndBlobs;
use buck2_execute::execute::action_digest_and_blobs::ActionDigestAndBlobsBuilder;
use buck2_execute::execute::write_action_cache::PopulateOutcome;
use buck2_execute::execute::write_action_cache::WriteActionCache;
use buck2_execute::materialize::materializer::CasDownloadInfo;
use buck2_execute::materialize::materializer::DeclareArtifactPayload;
use buck2_execute::materialize::materializer::WriteRequest;
use buck2_execute::re::ttl::re_expiration_from_ttl;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_hash::BuckMutMap;
use buck2_util::strong_hasher::Blake3StrongHasher;
use dupe::Dupe;
use jiff::Timestamp;
use remote_execution as RE;
use starlark::values::UnpackValue;
use starlark::values::Value;
use strong_hash::StrongHash;

use crate::actions::impls::produce;

/// Bump when anything about how a `WriteCacheKey` is computed changes: an entry recorded under
/// one scheme must not be found under another.
const KEY_VERSION: u32 = 0;

/// The first argument of every entry's synthetic action. No rule runs a program by this name, so
/// no entry's digest is one a real action could have, or the reverse.
const ACTION_MARKER: &str = "buck2-write-action-cache";

#[derive(Copy, Clone, Dupe)]
pub(crate) struct WriteCache<'a> {
    cache: &'a dyn WriteActionCache,
    mode: WriteActionCacheMode,
    /// The keys whose misses are recorded; none where the executor publishes no local results.
    populate: RolloutFraction,
}

/// What a write does with the cache.
#[derive(Copy, Clone, Dupe, PartialEq, Eq)]
pub(crate) enum CacheUse {
    /// Nothing: the write is declared as it is without the cache.
    None,
    /// Served from the cache on a hit; a miss leaves no entry.
    Lookup,
    /// Served from the cache on a hit; a miss is recorded for later builds.
    Populate,
}

/// Identifies a write's content without rendering it.
pub(crate) struct WriteCacheKey(blake3::Hash);

impl WriteCacheKey {
    pub(crate) fn to_hex(&self) -> String {
        self.0.to_hex().to_string()
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

/// Appends a write's key to the file at `path`, one `<owner>\t<output>\t<key>` line; the golden
/// keys in `test_write_action_cache` are read off this (`BUCK2_TEST_WRITE_ACTION_CACHE_KEY_DUMP`).
pub(crate) fn dump_key(
    path: &str,
    owner: &DeferredHolderKey,
    output: &ForwardRelativePath,
    key: &WriteCacheKey,
) -> buck2_error::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .buck_error_context("Opening the write action cache key dump")?;
    // One write per line: writes run concurrently and append to the same file.
    let line = format!("{}\t{}\t{}\n", owner.owner(), output, key.to_hex());
    file.write_all(line.as_bytes())
        .buck_error_context("Writing the write action cache key dump")?;
    Ok(())
}

/// What decides a write's content besides its command line, matching what
/// `WriteCommandLineOptions::render` is given, and what the content is declared with.
pub(crate) struct KeyInputs<'a> {
    pub(crate) is_executable: bool,
    /// The execution platform's, which rendering puts between path components.
    pub(crate) path_separator: PathSeparatorKind,
    pub(crate) macro_files: Option<&'a [Artifact]>,
    /// `None` when the content is rendered with `DepFilesPlaceholderArtifactPathMapper`.
    pub(crate) artifact_path_mapping: Option<&'a BuckMutMap<&'a Artifact, ContentBasedPathHash>>,
}

impl<'a> WriteCache<'a> {
    /// The cache this action may go through; `None` where none of its writes would ask it.
    pub(crate) fn for_ctx(ctx: &'a dyn ActionExecutionCtx) -> Option<Self> {
        if !ctx.may_probe_cas() {
            return None;
        }
        let cache = ctx.write_action_cache()?;
        let knobs = ctx.run_action_knobs();
        // An entry recorded is a local result published, so the share is read under the rule a
        // run's result is published by.
        let populate = if cache.populates() {
            knobs.write_action_cache_populate
        } else {
            RolloutFraction::none()
        };
        if knobs.write_action_cache == WriteActionCacheMode::Off && populate.is_none() {
            return None;
        }
        Some(Self {
            cache,
            mode: knobs.write_action_cache,
            populate,
        })
    }

    /// Whether writes are rendered regardless and hits only checked against them.
    pub(crate) fn verifies(self) -> bool {
        matches!(self.mode, WriteActionCacheMode::Verify)
    }

    /// What the write with `key` does with the cache.
    pub(crate) fn use_for(self, key: &WriteCacheKey) -> CacheUse {
        if self.populate.includes(key.as_bytes()) {
            CacheUse::Populate
        } else if self.mode == WriteActionCacheMode::Use {
            CacheUse::Lookup
        } else {
            CacheUse::None
        }
    }

    /// The content the cache holds for `key`. `None` is a miss, which a cache that could not
    /// answer or holds a malformed entry also is; both are reported once.
    pub(crate) async fn lookup(
        self,
        ctx: &dyn ActionExecutionCtx,
        key: &WriteCacheKey,
    ) -> Option<CachedContent> {
        let action = action(key, ctx);
        let use_case = self.cache.use_case();
        let result = match self.cache.lookup(&action).await {
            Ok(Some(result)) => result,
            Ok(None) => {
                WriteActionCacheStats::bump(&WRITE_ACTION_CACHE_STATS.misses);
                return None;
            }
            Err(e) => {
                WriteActionCacheStats::bump(&WRITE_ACTION_CACHE_STATS.lookup_failures);
                soft_error_or_panic!(
                    "write_action_cache_lookup_failed",
                    e.context(
                        "Action cache lookup for a write's content failed; producing it instead"
                    )
                );
                return None;
            }
        };
        let digest = match (result.exit_code, result.output_files.as_slice()) {
            (0, [file]) => FileDigest::from_re(&file.digest.digest, ctx.digest_config())
                .map(|digest| (digest, file.ttl))
                .map_err(buck2_error::Error::from),
            _ => Err(buck2_error!(
                buck2_error::ErrorTag::Tier0,
                "Expected one successful output, got exit code {} and {} outputs",
                result.exit_code,
                result.output_files.len()
            )),
        };
        let (digest, ttl) = match digest {
            Ok(digest) => digest,
            Err(e) => {
                soft_error_or_panic!(
                    "write_action_cache_bad_entry",
                    e.context(format!(
                        "Action cache entry `{}` for a write is malformed; producing the content instead",
                        action.action
                    )));
                return None;
            }
        };
        let now = Timestamp::now();
        let expires = re_expiration_from_ttl(now, ttl, &action.action);
        let digest = TrackedFileDigest::new_expires(
            digest,
            expires,
            ctx.digest_config().cas_digest_config(),
        );
        let info = CasDownloadInfo::new_execution(
            action.action,
            use_case,
            now,
            expires.duration_since(now),
        );
        WriteActionCacheStats::bump(&WRITE_ACTION_CACHE_STATS.hits);
        Some(CachedContent {
            digest,
            info: Arc::new(info),
        })
    }

    /// Records `request`'s content as what `key` identifies, uploading it to the CAS, and returns
    /// it with the download info to declare it under.
    pub(crate) async fn populate(
        self,
        ctx: &dyn ActionExecutionCtx,
        key: &WriteCacheKey,
        request: &WriteRequest,
    ) -> buck2_error::Result<(ArtifactValue, Arc<CasDownloadInfo>)> {
        let digest_config = ctx.digest_config();
        let digest =
            TrackedFileDigest::from_content(&request.content, digest_config.cas_digest_config());
        let action = action(key, ctx);
        match self
            .cache
            .populate(
                &action,
                &request.content,
                &digest,
                request.is_executable,
                digest_config,
            )
            .await?
        {
            PopulateOutcome::Populated => {}
            PopulateOutcome::PermissionDenied { reason } => {
                return Err(buck2_error!(
                    buck2_error::ErrorTag::Tier0,
                    "Not permitted to write the action cache: {reason}"
                ));
            }
        }
        WriteActionCacheStats::bump(&WRITE_ACTION_CACHE_STATS.populated);

        let value = ArtifactValue::file(FileMetadata {
            digest,
            is_executable: request.is_executable,
        });
        Ok((
            value,
            Arc::new(CasDownloadInfo::new_uploaded(self.cache.use_case())),
        ))
    }
}

/// Content the cache holds for a write: its digest, carrying the expiration the entry reported,
/// and the download info to declare it under.
pub(crate) struct CachedContent {
    pub(crate) digest: TrackedFileDigest,
    pub(crate) info: Arc<CasDownloadInfo>,
}

impl CachedContent {
    pub(crate) fn value(&self, is_executable: bool) -> ArtifactValue {
        ArtifactValue::file(FileMetadata {
            digest: self.digest.dupe(),
            is_executable,
        })
    }
}

/// Declares `value`, CAS content per `info`, as the write's output at `output`.
pub(crate) async fn declare(
    ctx: &dyn ActionExecutionCtx,
    output: &BuildArtifact,
    value: ArtifactValue,
    info: Arc<CasDownloadInfo>,
) -> buck2_error::Result<()> {
    let path = ctx.fs().resolve_build(
        output.get_path(),
        output
            .get_path()
            .is_content_based_path()
            .then(|| value.content_based_path_hash())
            .as_ref(),
    )?;
    ctx.materializer()
        .declare_cas_many(
            info,
            vec![DeclareArtifactPayload {
                path,
                artifact: value,
            }],
        )
        .await
}

/// Declares `request`, a write the cache missed, as the write's output: made here or uploaded,
/// as `produce::produce_write` says. Returns the value with `Simple` for a write made here and
/// `Deferred` for one declared from the CAS or found on disk.
pub(crate) async fn declare_miss(
    ctx: &dyn ActionExecutionCtx,
    request: WriteRequest,
) -> buck2_error::Result<(ArtifactValue, ActionExecutionKind)> {
    let (value, kind) = produce::produce_write(ctx, request).await?;
    if matches!(kind, ActionExecutionKind::Simple) {
        WriteActionCacheStats::bump(&WRITE_ACTION_CACHE_STATS.written);
    }
    Ok((value, kind))
}

/// The key for the content `WriteCommandLineOptions::render` would produce from `contents` and
/// `inputs`. The hashing builder hashes artifacts and cell paths by identity, so where rendering
/// puts `buck-out` and the cells goes in here, as does the mapping rendering resolves
/// content-based paths through.
pub(crate) fn key(
    contents: Value<'_>,
    inputs: &KeyInputs<'_>,
    fs: &ArtifactFs,
) -> buck2_error::Result<WriteCacheKey> {
    let mut hasher = Blake3StrongHasher::new();
    hasher.write_u32(KEY_VERSION);

    hash_str(&mut hasher, fs.buck_out_path_resolver().root().as_str());
    let mut cells: Vec<(&str, &str)> = fs
        .cell_resolver()
        .cells()
        .map(|(name, instance)| (name.as_str(), instance.path().as_str()))
        .collect();
    cells.sort();
    hasher.write_usize(cells.len());
    for (name, root) in cells {
        hash_str(&mut hasher, name);
        hash_str(&mut hasher, root);
    }

    ValueAsCommandLineLike::unpack_value_err(contents)?
        .0
        .add_to_command_line(&mut CommandLineBuilder::new_hashing(&mut hasher))?;

    hasher.write_u8(inputs.is_executable as u8);
    hasher.write_u8(match inputs.path_separator {
        PathSeparatorKind::Unix => 0,
        PathSeparatorKind::Windows => 1,
    });

    // The hashing builder stands in a marker for each write-to-file macro path; rendering puts
    // the macro files' paths there, in order.
    match inputs.macro_files {
        None => hasher.write_u8(0),
        Some(macro_files) => {
            hasher.write_u8(1);
            hasher.write_usize(macro_files.len());
            for artifact in macro_files {
                artifact.strong_hash(&mut hasher);
            }
        }
    }

    // Each content-based artifact with the hash rendering resolves it to, in an order that does
    // not depend on the map's.
    match inputs.artifact_path_mapping {
        None => hasher.write_u8(0),
        Some(mapping) => {
            hasher.write_u8(1);
            let mut entries: Vec<[u8; 32]> = mapping
                .iter()
                .map(|(artifact, hash)| {
                    let mut entry = Blake3StrongHasher::new();
                    artifact.strong_hash(&mut entry);
                    hash_str(&mut entry, hash.as_str());
                    *entry.finalize().as_bytes()
                })
                .collect();
            entries.sort_unstable();
            hasher.write_usize(entries.len());
            for entry in entries {
                hasher.write(&entry);
            }
        }
    }

    Ok(WriteCacheKey(hasher.finalize()))
}

fn hash_str(hasher: &mut Blake3StrongHasher, s: &str) {
    hasher.write_usize(s.len());
    hasher.write(s.as_bytes());
}

/// The synthetic action an entry for `key` lives under. Like a remote dep file's, it exists so
/// the entry has an action digest, and the action cache requires the action to be in the CAS.
fn action(key: &WriteCacheKey, ctx: &dyn ActionExecutionCtx) -> ActionDigestAndBlobs {
    let mut blobs = ActionDigestAndBlobsBuilder::new(ctx.digest_config());
    let command = blobs.add_command(&RE::Command {
        arguments: vec![ACTION_MARKER.to_owned(), key.0.to_hex().to_string()],
        #[allow(deprecated)]
        platform: Some(ctx.re_platform().clone()),
        ..Default::default()
    });
    blobs.build(&RE::Action {
        command_digest: Some(command.to_grpc()),
        ..Default::default()
    })
}
