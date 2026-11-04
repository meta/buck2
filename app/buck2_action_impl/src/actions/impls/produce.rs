/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! How content an action makes, a write or a copy of a source file, comes to be once the CAS
//! probe has missed it: the way a run action's output does. Made here under a permit of the gate
//! the hybrid executor runs local commands behind, held while it is made so that a burst counts
//! as the load it is; uploaded and declared as CAS content when there is no permit to be had,
//! since a burst of writes in a cold build is thousands of files nothing on this machine may
//! ever read, and one write in a small build is quicker made than sent. "Here" is a declaration
//! under the deferred core; under the disk-state core, `produce_locally`, which finds a previous
//! build's content on disk rather than make it again, or for a write under
//! `defer_write_actions`, the BlobStore's memory, where the content waits, compressed, for
//! something to ask for the file.

use std::sync::Arc;

use buck2_build_api::actions::ActionExecutionCtx;
use buck2_build_api::actions::execute::action_executor::ActionExecutionKind;
use buck2_build_api::actions::impls::write_action_cache_stats::WRITE_ACTION_CACHE_STATS;
use buck2_build_api::actions::impls::write_action_cache_stats::WriteActionCacheStats;
use buck2_common::cas_digest::CasDigestConfig;
use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::FileDigestConfig;
use buck2_common::file_ops::metadata::FileMetadata;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_directory::directory::directory::Directory;
use buck2_directory::directory::directory_iterator::DirectoryIterator;
use buck2_directory::directory::directory_iterator::DirectoryIteratorPathStack;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_directory::directory::walk::unordered_entry_walk;
use buck2_error::BuckErrorOptionContext;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::blob_store::BlobStore;
use buck2_execute::directory::ActionDirectoryBuilder;
use buck2_execute::directory::ActionDirectoryEntry;
use buck2_execute::directory::ActionDirectoryMember;
use buck2_execute::directory::ActionSharedDirectory;
use buck2_execute::execute::blobs::ActionBlobs;
use buck2_execute::execute::clean_output_paths::cleanup_path;
use buck2_execute::execute::low_pass_filter::LowPassFilterGuard;
use buck2_execute::materialize::materializer::CasDownloadInfo;
use buck2_execute::materialize::materializer::DeclareArtifactPayload;
use buck2_execute::materialize::materializer::WriteRequest;
use buck2_execute::materialize::produce::Produced;
use buck2_execute::materialize::produce::produce_locally;
use buck2_execute::materialize::produce::write_locally;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use dupe::Dupe;

use crate::actions::impls::cas_probe::SourceFile;
use crate::actions::impls::cas_probe::files_of;

/// The weight of one piece of content behind the gate: small, short local work.
const LOCAL_WEIGHT: usize = 1;

/// A permit for one piece of local work if there is room for it right now, else nothing: what
/// decides between making content here and sending it.
fn room(ctx: &dyn ActionExecutionCtx) -> Option<LowPassFilterGuard<'_>> {
    ctx.low_pass_filter().try_access(LOCAL_WEIGHT)
}

/// `Simple` for content made here, `Deferred` for content the CAS or the disk held already.
fn kind(produced: Produced) -> ActionExecutionKind {
    match produced {
        Produced::Made => ActionExecutionKind::Simple,
        Produced::Found => ActionExecutionKind::Deferred,
    }
}

/// A write the CAS lacks: uploaded and declared as CAS content when there is no room for local
/// work, else made here. Returns the value with `Simple` where the write was made here and
/// `Deferred` where the CAS holds it, by upload, or the path held it already.
pub(crate) async fn produce_write(
    ctx: &dyn ActionExecutionCtx,
    request: WriteRequest,
) -> buck2_error::Result<(ArtifactValue, ActionExecutionKind)> {
    let permit = room(ctx);
    if permit.is_none() {
        if let Some((value, info)) = upload_write(ctx, &request).await {
            ctx.materializer()
                .declare_cas_many(
                    info,
                    vec![DeclareArtifactPayload {
                        path: request.path,
                        artifact: value.dupe(),
                    }],
                )
                .await?;
            return Ok((value, ActionExecutionKind::Deferred));
        }
    }
    let (value, produced) = write_here(ctx, request).await?;
    drop(permit);
    Ok((value, kind(produced)))
}

/// A write made on this machine, whichever core: under the disk-state core, kept in memory
/// (`defer_write`) or on disk now, found or made (`write_locally`); a declaration under the
/// deferred core, written when something reads it, or at once, as that core's own
/// `defer_write_actions` says.
pub(crate) async fn write_here(
    ctx: &dyn ActionExecutionCtx,
    request: WriteRequest,
) -> buck2_error::Result<(ArtifactValue, Produced)> {
    let materializer = ctx.materializer();
    if materializer.is_disk_state() {
        if ctx.run_action_knobs().defer_write_actions {
            return defer_write(ctx, request);
        }
        return write_locally(
            materializer,
            ctx.blocking_executor(),
            ctx.fs().fs(),
            ctx.digest_config(),
            request,
        )
        .await;
    }
    let value = materializer
        .declare_write(Box::new(move || Ok(vec![request])))
        .await?
        .into_iter()
        .next()
        .internal_error("a write declares one value")?;
    Ok((value, Produced::Made))
}

/// A write whose content goes into the BlobStore's memory, compressed, rather than to its path:
/// the value names the digest, and the materializer writes the file out of memory when something
/// asks for the path, as it links one out of the stash. Nothing is on disk, and the materializer
/// hears nothing, until then.
fn defer_write(
    ctx: &dyn ActionExecutionCtx,
    request: WriteRequest,
) -> buck2_error::Result<(ArtifactValue, Produced)> {
    let WriteRequest {
        path: _,
        content,
        is_executable,
        path_kind: _,
    } = request;
    let digest = TrackedFileDigest::from_content(&content, ctx.digest_config().cas_digest_config());
    BlobStore::global().insert_in_memory(&content, &digest, is_executable)?;
    let value = ArtifactValue::file(FileMetadata {
        digest,
        is_executable,
    });
    Ok((value, Produced::Made))
}

/// Uploads a write's content and returns its value with the download info to declare it under.
/// `None` where the executor configuration has no CAS to upload to, or where the upload failed,
/// which is reported once: making the write here is the right fallback, but a CAS that refuses
/// uploads would silently turn every busy write into a local one.
async fn upload_write(
    ctx: &dyn ActionExecutionCtx,
    request: &WriteRequest,
) -> Option<(ArtifactValue, Arc<CasDownloadInfo>)> {
    let cache = ctx.write_action_cache()?;
    let digest_config = ctx.digest_config();
    let digest =
        TrackedFileDigest::from_content(&request.content, digest_config.cas_digest_config());
    if let Err(e) = cache.upload(&request.content, &digest, digest_config).await {
        buck2_core::soft_error_or_panic!(
            "write_action_cache_upload_failed",
            e.context("Uploading a write's content failed; writing it locally instead")
        );
        return None;
    }
    WriteActionCacheStats::bump(&WRITE_ACTION_CACHE_STATS.uploaded);
    let value = ArtifactValue::file(FileMetadata {
        digest,
        is_executable: request.is_executable,
    });
    Some((
        value,
        Arc::new(CasDownloadInfo::new_uploaded(cache.use_case())),
    ))
}

/// A copy of a source the CAS lacks: `value` at `dest`, copied from `src`.
pub(crate) struct SourceCopy {
    pub(crate) src: ProjectRelativePathBuf,
    pub(crate) dest: ProjectRelativePathBuf,
    pub(crate) value: ArtifactValue,
    pub(crate) executable_bit_override: Option<bool>,
}

/// Copies of sources the CAS lacks, under the disk-state core: each made here under a permit
/// while there is room for local work, the rest uploaded in one go, and what the CAS would not
/// take made here after all. With `uploadable` false, where the CAS cannot be asked, everything
/// is made here. Returns `Simple` where anything was made here, `Deferred` otherwise.
pub(crate) async fn produce_copies(
    ctx: &dyn ActionExecutionCtx,
    copies: Vec<SourceCopy>,
    uploadable: bool,
) -> buck2_error::Result<ActionExecutionKind> {
    let mut here = Vec::new();
    let mut permits = Vec::new();
    let mut busy = Vec::new();
    for copy in copies {
        match room(ctx) {
            Some(permit) => {
                permits.push(permit);
                here.push(copy);
            }
            None if uploadable => busy.push(copy),
            None => here.push(copy),
        }
    }
    if !busy.is_empty() {
        let files = busy
            .iter()
            .flat_map(|copy| files_of(copy.value.entry(), &copy.src))
            .collect();
        if !upload_sources(ctx, files).await {
            here.extend(busy);
        }
    }
    // One at a time: a copied directory the CAS lacks is thousands of files, and all of them at
    // once is the inline IO burst the gate exists to prevent.
    let mut made = Vec::with_capacity(here.len());
    for copy in here {
        made.push(copy_here(ctx, copy).await?);
    }
    drop(permits);
    Ok(if made.contains(&Produced::Made) {
        ActionExecutionKind::Simple
    } else {
        ActionExecutionKind::Deferred
    })
}

/// A copy made on this machine, found at its destination or made there from the source.
async fn copy_here(
    ctx: &dyn ActionExecutionCtx,
    copy: SourceCopy,
) -> buck2_error::Result<Produced> {
    let SourceCopy {
        src,
        dest,
        value,
        executable_bit_override,
    } = copy;
    let (fs, entry, config) = (
        ctx.fs().fs().dupe(),
        value.entry().dupe(),
        ctx.digest_config().cas_digest_config(),
    );
    let make_at = dest.clone();
    produce_locally(
        ctx.materializer(),
        ctx.blocking_executor(),
        dest,
        value,
        move || copy_entry(&fs, &src, &make_at, &entry, executable_bit_override, config),
    )
    .await
}

/// Copies the source entry at `src` to `dest`, the whole of it, with each file's content checked
/// against the digest the value claims: a source file the user changed since it was hashed fails
/// the copy, and the action runs again on the file watcher's invalidation.
fn copy_entry(
    fs: &ProjectRoot,
    src: &ProjectRelativePath,
    dest: &ProjectRelativePath,
    entry: &ActionDirectoryEntry<ActionSharedDirectory>,
    executable_bit_override: Option<bool>,
    config: CasDigestConfig,
) -> buck2_error::Result<()> {
    cleanup_path(fs, dest)?;
    let (src, dest) = (fs.resolve(src), fs.resolve(dest));
    let mut walk = unordered_entry_walk(entry.as_ref().map_dir(Directory::as_ref));
    while let Some((path, entry)) = walk.next() {
        let path = path.get();
        let (from, to) = (src.join(&path), dest.join(&path));
        if let Some(parent) = to.parent() {
            fs_util::create_dir_all(parent)?;
        }
        match entry {
            DirectoryEntry::Dir(_) => fs_util::create_dir_all(&to)?,
            DirectoryEntry::Leaf(ActionDirectoryMember::File(file)) => {
                fs_util::reflink_or_copy(&from, &to).categorize_internal()?;
                let found = FileDigest::from_file(&to, FileDigestConfig::source(config))?;
                if &found != file.digest.data() {
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "Source file `{from}` changed while it was being copied: expected {}, found {found}",
                        file.digest.data()
                    ));
                }
                let executable = executable_bit_override.unwrap_or(file.is_executable);
                fs_util::set_executable(&to, executable).categorize_internal()?;
            }
            DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(symlink)) => {
                // A relative path normalizes `.` away, so a link to its own directory has an
                // empty target, which the filesystem refuses.
                let target = match symlink.target().as_str() {
                    "" => ".",
                    target => target,
                };
                fs_util::symlink(target, &to).categorize_internal()?;
            }
            DirectoryEntry::Leaf(ActionDirectoryMember::ExternalSymlink(symlink)) => {
                fs_util::symlink(symlink.target(), &to).categorize_internal()?;
            }
        }
    }
    Ok(())
}

/// Puts `files`, source files the CAS lacks, into the CAS as an action's inputs are put there:
/// behind the CAS semaphore, read off disk. A failure is reported once and answered `false`,
/// which leaves the files for the caller to make here.
async fn upload_sources(ctx: &dyn ActionExecutionCtx, files: Vec<SourceFile>) -> bool {
    let digest_config = ctx.digest_config();
    // Non-exhaustive: a few source files laid out under the project root, not its contents.
    let mut builder = ActionDirectoryBuilder::empty_non_exhaustive();
    for (path, metadata) in &files {
        let inserted = builder.insert(
            path,
            DirectoryEntry::Leaf(ActionDirectoryMember::File(metadata.dupe())),
        );
        if let Err(e) = inserted {
            buck2_core::soft_error_or_panic!(
                "source_upload_failed",
                buck2_error::Error::from(e).context(format!("Laying out `{path}` for upload"))
            );
            return false;
        }
    }
    let dir = builder.fingerprint(digest_config.as_directory_serializer());
    let uploaded = ctx
        .re_client()
        .with_use_case(ctx.invocation_re_use_case())
        .upload(
            ctx.fs().fs(),
            ctx.materializer(),
            &ActionBlobs::new(digest_config),
            ProjectRelativePath::empty(),
            &dir,
            None,
        )
        .await;
    match uploaded {
        Ok(_) => true,
        Err(e) => {
            buck2_core::soft_error_or_panic!(
                "source_upload_failed",
                e.context("Uploading source files an action copies failed; copying them instead")
            );
            false
        }
    }
}
