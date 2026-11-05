/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Content an action can make, a write or a copy of a source file, that the CAS already holds is
//! declared as CAS content instead of being made. A write asks behind the `write_cas_probe`
//! knob; a copy of a source file asks under the disk-state core, which would otherwise make the
//! copy now. What the CAS lacks comes to be as `produce` says.

use std::sync::Arc;

use buck2_build_api::actions::ActionExecutionCtx;
use buck2_common::file_ops::metadata::FileMetadata;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_directory::directory::directory::Directory;
use buck2_directory::directory::directory_iterator::DirectoryIterator;
use buck2_directory::directory::directory_iterator::DirectoryIteratorPathStack;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_directory::directory::walk::unordered_entry_walk;
use buck2_execute::directory::ActionDirectoryEntry;
use buck2_execute::directory::ActionDirectoryMember;
use buck2_execute::directory::ActionSharedDirectory;
use buck2_execute::materialize::materializer::CasDownloadInfo;
use buck2_execute::re::presence::NegativeCache;
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
use dupe::Dupe;

/// A source file as the probe and the upload see it: where it is on disk, and what its copy says
/// it holds.
pub(crate) type SourceFile = (ProjectRelativePathBuf, FileMetadata);

/// Whether a write asks the CAS before its content is made: the knob's decision, and never when
/// the invocation asks the CAS nothing (`ActionExecutionCtx::may_probe_cas`).
pub(crate) fn probes_writes(ctx: &dyn ActionExecutionCtx) -> bool {
    ctx.run_action_knobs().write_cas_probe && ctx.may_probe_cas()
}

/// Whether a copy of a source file asks the CAS before it is made: under the disk-state core
/// always, since what a hit spares there is a copy made now, a file at a time; under the deferred
/// core never, since a copy there is a declaration made only when something needs the file, and a
/// hit would turn it into a download. Never when the invocation asks the CAS nothing.
pub(crate) fn probes_copies(ctx: &dyn ActionExecutionCtx) -> bool {
    ctx.materializer().is_disk_state() && ctx.may_probe_cas()
}

/// Which of `digests` the CAS holds, one answer per digest, with the `CasDownloadInfo` to
/// declare the hits under; a hit's digest carries the expiration the check found. `None` when
/// the CAS could not answer, which is reported once and is every digest missing to the caller.
pub(crate) async fn probe(
    ctx: &dyn ActionExecutionCtx,
    digests: Vec<TrackedFileDigest>,
) -> Option<(Arc<CasDownloadInfo>, Vec<bool>)> {
    let use_case = ctx.invocation_re_use_case();
    let info = Arc::new(CasDownloadInfo::new_probed(use_case));
    // A stale miss only costs producing what would otherwise have been skipped.
    let presence = match ctx
        .re_client()
        .with_use_case(use_case)
        .check_presence(digests, NegativeCache::Allowed, &info)
        .await
    {
        Ok(presence) => presence,
        Err(e) => {
            // Producing the content is the right fallback, but a CAS that cannot answer would
            // silently turn every probe into a miss, which should be visible.
            buck2_core::soft_error_or_panic!(
                "write_cas_probe_failed",
                e.context("CAS lookup for content an action produces failed; producing it instead")
            );
            return None;
        }
    };
    let in_cas = presence
        .into_iter()
        .map(|presence| presence.is_some())
        .collect();
    Some((info, in_cas))
}

/// The probe over `files`, source files a copy would make: the info to declare the hits under,
/// and the positions of the files the CAS lacks. `None` when the CAS was not asked
/// (`probes_copies`) or could not answer, in which case every file is the caller's to make.
pub(crate) async fn missing(
    ctx: &dyn ActionExecutionCtx,
    files: &[SourceFile],
) -> Option<(Arc<CasDownloadInfo>, Vec<usize>)> {
    if !probes_copies(ctx) {
        return None;
    }
    let digests = files
        .iter()
        .map(|(_, metadata)| metadata.digest.dupe())
        .collect();
    let (info, in_cas) = probe(ctx, digests).await?;
    let missing = in_cas
        .into_iter()
        .enumerate()
        .filter_map(|(i, in_cas)| (!in_cas).then_some(i))
        .collect();
    Some((info, missing))
}

/// The files of `entry`, at their paths within it, in walk order.
pub(crate) fn walk_files(
    entry: &ActionDirectoryEntry<ActionSharedDirectory>,
) -> Vec<(ForwardRelativePathBuf, FileMetadata)> {
    let mut files = Vec::new();
    let mut walk = unordered_entry_walk(entry.as_ref().map_dir(Directory::as_ref));
    while let Some((path, entry)) = walk.next() {
        if let DirectoryEntry::Leaf(ActionDirectoryMember::File(file)) = entry {
            files.push((path.get(), file.dupe()));
        }
    }
    files
}

/// The files of `entry`, at their paths under `root`, in walk order.
pub(crate) fn files_of(
    entry: &ActionDirectoryEntry<ActionSharedDirectory>,
    root: &ProjectRelativePath,
) -> Vec<SourceFile> {
    walk_files(entry)
        .into_iter()
        .map(|(path, metadata)| (root.join(&path), metadata))
        .collect()
}
