/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_core::fs::i_project_rel_path::IProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::soft_error;
use buck2_error::BuckErrorContext;
use buck2_error::internal_error;
use buck2_fs::paths::file_name::FileName;
use buck2_hash::BuckDashMap;
use buck2_hash::BuckMutSet;
use buck2_util::arc_str::PackedStringSlice;
use buck2_util::arc_str::PackedStringSliceBuilder;
use derive_more::Display;
use dice::CancellationContext;
use dice::DiceComputations;
use dice::DiceTransactionUpdater;
use dice::EqualityBehavior;
use dice::InjectedKey;
use dice::InvalidationSourcePriority;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::PagableValueSerialize;
use dice::UserComputationData;
use dice::ValueSerialize;
use dupe::Dupe;
use dupe::ResultDupedErrExt;
use pagable::Pagable;
use pagable::pagable_typetag;
use tokio::sync::OnceCell;

use crate::dice::data::HasIoProvider;
use crate::file_ops::baseline::HasFileSystemBaseline;
use crate::file_ops::metadata::FileType;
use crate::file_ops::metadata::RawPathMetadata;
use crate::file_ops::metadata::RawSymlink;
use crate::file_ops::path_check::PathCheck;
use crate::file_ops::path_check::PathCheckError;
use crate::file_ops::path_check::PathCheckKey;
use crate::file_ops::path_check::ReadKind;
use crate::file_ops::path_check::check_ignored_path;
use crate::file_ops::path_check::check_path;
use crate::file_ops::path_check::read_ignored_path;
use crate::ignores::ignore_set::HasIgnoreSet;
use crate::ignores::ignore_set::IgnoreSet;
use crate::io::trace;
use crate::io::trace::TracingIoProvider;
use crate::legacy_configs::configs::LegacyBuckConfig;
use crate::legacy_configs::key::BuckconfigKeyRef;

/// The `Io*` keys are the DICE nodes at which file system state enters the graph; file watchers
/// invalidate them through the `FileChangeTracker`.
///
/// Each of them depends on the [`FileSystemBaseline`](crate::file_ops::baseline::FileSystemBaseline)
/// so that nothing read under one baseline is reused under another.
///
/// The contents of a file are read by the caller, after computing this key, so that they are never
/// held in the graph. The key is therefore never equal to its previous value: a change to the file
/// must recompute whoever read it.
#[derive(Clone, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct IoReadFileKey(IProjectRelativePath);

#[async_trait]
impl Key for IoReadFileKey {
    type Value = buck2_error::Result<PathCheck>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        ctx.get_file_system_baseline().await?;
        check_path(ctx, self.0).await
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::AlwaysUnequal
    }

    fn invalidation_source_priority() -> InvalidationSourcePriority {
        InvalidationSourcePriority::High
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        OkPagableValueSerialize::<Self::Value>::new()
    }
}

/// What listing a directory produced.
#[derive(Clone, Dupe, Debug, PartialEq, Eq, Allocative, Pagable)]
pub(super) enum DirListing {
    Entries(PackedStringSlice<FileName, FileType>),
    /// The check found something other than a present directory.
    Refused(PathCheck),
}

#[derive(Clone, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct IoReadDirKey(IProjectRelativePath);

#[async_trait]
impl Key for IoReadDirKey {
    type Value = buck2_error::Result<DirListing>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        // Not redundant with the check's own dependency on the baseline: the check can come out
        // equal under a new baseline while the listing changed.
        ctx.get_file_system_baseline().await?;
        match check_path(ctx, self.0).await? {
            PathCheck::Present(FileType::Directory) => {}
            check @ (PathCheck::Present(FileType::Symlink) | PathCheck::ParentSymlink { .. }) => {
                if !read_through_symlink(ctx, self.0, check, ReadKind::Directory).await? {
                    return Ok(DirListing::Refused(check));
                }
            }
            check => return Ok(DirListing::Refused(check)),
        }
        let entries = read_dir_cached(ctx, self.0).await?;
        Ok(DirListing::Entries(without_ignored(
            self.0,
            entries,
            &ctx.global_data().get_ignore_set(),
        )))
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| match (x, y) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        })
    }

    fn validity(x: &Self::Value) -> bool {
        x.is_ok()
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        OkPagableValueSerialize::<Self::Value>::new()
    }
}

/// `entries`, listed at `dir`, minus those matching the ignore set.
fn without_ignored(
    dir: IProjectRelativePath,
    entries: PackedStringSlice<FileName, FileType>,
    ignores: &IgnoreSet,
) -> PackedStringSlice<FileName, FileType> {
    let mut buf = String::with_capacity(dir.as_str().len() + 1 + 32);
    let mut is_ignored = |file_name: &FileName| {
        buf.clear();
        if !dir.is_empty() {
            buf.push_str(dir.as_str());
            buf.push('/');
        }
        buf.push_str(file_name.as_str());
        ignores.is_match(ProjectRelativePath::unchecked_new(&buf))
    };

    // Input is sorted; we only remove entries, never reorder, so the output stays sorted. Defer
    // allocating a builder until we encounter the first ignored entry, so that the common
    // no-ignored-files case can reuse the existing alloc.
    let mut builder: Option<PackedStringSliceBuilder<FileName, FileType>> = None;
    for (idx, (file_name, file_type)) in entries.iter_with_tag().enumerate() {
        if is_ignored(file_name) {
            builder.get_or_insert_with(|| {
                let mut b = PackedStringSliceBuilder::with_capacity(entries.len() - 1);
                for (fname, ftype) in entries.iter_with_tag().take(idx) {
                    b.push(fname, ftype);
                }
                b
            });
            continue;
        }
        if let Some(b) = builder.as_mut() {
            b.push(file_name, file_type);
        }
    }
    match builder {
        Some(b) => b.build_sorted(),
        None => entries,
    }
}

/// Lists a directory, at most once per transaction. The listing is raw: ignored entries included.
///
/// Reading through this rather than `IoReadDirKey` records no dependency on the listing; callers
/// that use it must be invalidated by the file watcher themselves.
pub(super) async fn read_dir_cached(
    ctx: &mut DiceComputations<'_>,
    path: IProjectRelativePath,
) -> buck2_error::Result<PackedStringSlice<FileName, FileType>> {
    // Commands set the cache up (`initialize_read_dir_cache`); without it every read goes to the
    // provider, which is correct and what unit tests get.
    let ctx: &DiceComputations<'_> = ctx;
    let Ok(read_dir_cache) = ctx.per_transaction_data().data.get::<ReadDirCache>() else {
        return list_dir(ctx, path).await;
    };
    // `get` avoids the shard write lock on a hit; `entry().or_default()` takes it only on a miss.
    let cell = read_dir_cache
        .0
        .get(&path)
        .map(|cell| cell.dupe())
        .unwrap_or_else(|| read_dir_cache.0.entry(path).or_default().dupe());
    // Mutually exclusive initialization to prevent multiple requests repeating directory listings.
    cell.get_or_init(|| list_dir(ctx, path)).await.dupe()
}

async fn list_dir(
    ctx: &DiceComputations<'_>,
    path: IProjectRelativePath,
) -> buck2_error::Result<PackedStringSlice<FileName, FileType>> {
    let entries = ctx
        .global_data()
        .get_io_provider()
        .read_dir(path)
        .await
        .with_buck_error_context(|| format!("Error listing dir `{}`", path))?
        .into_entries();

    // Pack into a sorted compact representation. `read_dir` is not guaranteed
    // to return entries in any particular order.
    let mut builder: PackedStringSliceBuilder<FileName, FileType> =
        PackedStringSliceBuilder::with_capacity(entries.len());
    for e in &entries {
        builder.push(&e.file_name, e.file_type);
    }
    Ok(builder.sort_and_build())
}

/// Which reads through symlinks are refused; the others go ahead, reported as soft errors.
///
/// This is how a repo that the soft errors of `read_through_symlink` say is clean turns the
/// refusal on. A symlink out of the repo, such as one into `/mnt/gvfs`, is told apart from one
/// into the repo because the two are cleaned up differently, and not at the same time.
///
/// A read is of one kind or the other by the first symlink on its path, which is the one the
/// path check finds: a path that reaches a symlink out of the repo through one into it counts as
/// into the repo. `out_of_repo` on its own therefore does not keep every read inside the repo;
/// together with `into_repo` it does.
#[derive(Clone, Copy, Dupe, Debug, Default, PartialEq, Eq, Allocative, Pagable)]
pub struct RefusedSymlinkReads {
    /// `buck2.refuse_reads_through_symlinks_into_repo`
    pub into_repo: bool,
    /// `buck2.refuse_reads_through_symlinks_out_of_repo`
    pub out_of_repo: bool,
}

impl RefusedSymlinkReads {
    pub fn from_root_buckconfig(config: &LegacyBuckConfig) -> buck2_error::Result<Self> {
        let is_set = |property| {
            buck2_error::Ok(
                config
                    .parse::<bool>(BuckconfigKeyRef {
                        section: "buck2",
                        property,
                    })?
                    .unwrap_or(false),
            )
        };
        Ok(Self {
            into_repo: is_set("refuse_reads_through_symlinks_into_repo")?,
            out_of_repo: is_set("refuse_reads_through_symlinks_out_of_repo")?,
        })
    }
}

/// Injected by each command. Reading the buckconfig on DICE from here is not an option:
/// buckconfigs are themselves read through this layer, so the read of one behind a symlink would
/// depend on itself.
#[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("{:?}", self)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct RefusedSymlinkReadsKey;

impl InjectedKey for RefusedSymlinkReadsKey {
    type Value = RefusedSymlinkReads;

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        PagableValueSerialize::<Self::Value>::new()
    }
}

pub trait SetRefusedSymlinkReads {
    /// Has to be set before anything is read through DICE.
    fn set_refused_symlink_reads(
        &mut self,
        refused: RefusedSymlinkReads,
    ) -> buck2_error::Result<()>;
}

impl SetRefusedSymlinkReads for DiceTransactionUpdater {
    fn set_refused_symlink_reads(
        &mut self,
        refused: RefusedSymlinkReads,
    ) -> buck2_error::Result<()> {
        Ok(self.changed_to(vec![(RefusedSymlinkReadsKey, refused)])?)
    }
}

/// Whether a read that found a symlink at the path or above it goes ahead anyway. If not, the
/// caller refuses the read with the check, as it does when the check finds anything else that is
/// not what the read wanted.
///
/// The read goes ahead, reported as a soft error, unless [`RefusedSymlinkReads`] says otherwise.
/// The soft errors show where such reads still happen, in categories that tell symlinks into the
/// repo from symlinks out of it. The extra logview samples are keyed by the symlink, which makes
/// them the list of what to clean up.
///
/// Once both kinds are refused everywhere, this function goes, along with the buckconfigs and the
/// code that reads through.
async fn read_through_symlink(
    ctx: &mut DiceComputations<'_>,
    path: IProjectRelativePath,
    check: PathCheck,
    kind: ReadKind,
) -> buck2_error::Result<bool> {
    let link = match check {
        PathCheck::ParentSymlink { at } => at,
        _ => path,
    };
    let external = matches!(
        ctx.compute(&IoPathMetadataKey(link)).await?.dupe()?,
        Some(RawPathMetadata::Symlink {
            to: RawSymlink::External(_),
            ..
        })
    );
    let refused = ctx.compute(&RefusedSymlinkReadsKey).await?;
    let refuse = if external {
        refused.out_of_repo
    } else {
        refused.into_repo
    };
    if refuse {
        return Ok(false);
    }
    let category = match (check, external) {
        (PathCheck::ParentSymlink { .. }, false) => "path_check_symlink_in_parent_two",
        (PathCheck::ParentSymlink { .. }, true) => "path_check_external_symlink_in_parent_two",
        (_, false) => "path_check_symlink_at_path_two",
        (_, true) => "path_check_external_symlink_at_path_two",
    };
    soft_error!(
        category,
        PathCheckError::new(ctx, path, check, kind).into(),
        low_cardinality_key_for_additional_logview_samples: Some(Box::new(link.as_str().to_owned())),
    );
    Ok(true)
}

#[derive(Clone, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct IoPathMetadataKey(IProjectRelativePath);

#[async_trait]
impl Key for IoPathMetadataKey {
    type Value = buck2_error::Result<Option<RawPathMetadata<IProjectRelativePath>>>;
    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        ctx.get_file_system_baseline().await?;
        let check = match check_path(ctx, self.0).await? {
            PathCheck::Ignored { at } => {
                let check = check_ignored_path(ctx, self.0, at).await?;
                // A directory is refused when it is listed, and a path behind a symlink is
                // reported once, by the symlink's own key.
                if let PathCheck::Present(file_type) = check
                    && file_type != FileType::Directory
                {
                    read_ignored_path(ctx, self.0, at)?;
                }
                check
            }
            check => check,
        };
        let at = match check {
            PathCheck::Present(FileType::Directory) => return Ok(Some(RawPathMetadata::Directory)),
            PathCheck::Present(_) => {
                let meta = ctx
                    .global_data()
                    .get_io_provider()
                    .read_path_metadata_if_exists(self.0)
                    .await
                    .with_buck_error_context(|| {
                        format!("Error accessing metadata for path `{p}`", p = self.0)
                    })?;
                return Ok(meta.map(|meta| meta.map(|p| IProjectRelativePath::intern_directly(&p))));
            }
            PathCheck::Missing { .. } | PathCheck::ParentNotDirectory { .. } => return Ok(None),
            PathCheck::Ignored { .. } => {
                return Err(internal_error!("ignored paths are looked at above"));
            }
            PathCheck::ParentSymlink { at } => at,
        };
        // Behind a symlink: report the symlink, with the rest of the path appended to its target,
        // which is what source artifacts through a symlink expect to see.
        let rest = self
            .0
            .strip_prefix(at)
            .internal_error("the check reported a prefix")?;
        match ctx.compute(&IoPathMetadataKey(at)).await?.dupe()? {
            Some(RawPathMetadata::Symlink { at: link, to }) if link == at => {
                let to = to.continued_by(rest)?;
                // The trace sees the provider's reads, and this one did not reach the provider.
                if let Some(trace) = TracingIoProvider::from_io(ctx.global_data().get_io_provider())
                {
                    trace.add_symlink(trace::Symlink {
                        at: at.to_buf(),
                        to: to.clone().map(|p| p.to_buf()),
                    });
                }
                Ok(Some(RawPathMetadata::Symlink { at, to }))
            }
            _ => Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::IoSource,
                "`{at}` stopped being a symlink while `{p}` was read",
                p = self.0
            )),
        }
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| match (x, y) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        })
    }

    fn validity(x: &Self::Value) -> bool {
        x.is_ok()
    }

    fn invalidation_source_priority() -> InvalidationSourcePriority {
        InvalidationSourcePriority::High
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        OkPagableValueSerialize::<Self::Value>::new()
    }
}

/// The contents of the file at the path, or the check that found no file there. A symlink at the
/// path or above it is read through if `read_through_symlink` says so.
pub(super) async fn read_file(
    ctx: &mut DiceComputations<'_>,
    path: IProjectRelativePath,
) -> buck2_error::Result<Result<String, PathCheck>> {
    let check = *ctx
        .compute(&IoReadFileKey(path))
        .await?
        .as_ref()
        .duped_err()?;
    match check {
        PathCheck::Present(FileType::File | FileType::Unknown) => {
            read_present_file(ctx, path).await
        }
        check @ (PathCheck::Present(FileType::Symlink) | PathCheck::ParentSymlink { .. }) => {
            if read_through_symlink(ctx, path, check, ReadKind::File).await? {
                read_present_file(ctx, path).await
            } else {
                Ok(Err(check))
            }
        }
        check => Ok(Err(check)),
    }
}

async fn read_present_file(
    ctx: &mut DiceComputations<'_>,
    path: IProjectRelativePath,
) -> buck2_error::Result<Result<String, PathCheck>> {
    let contents = ctx
        .global_data()
        .get_io_provider()
        .read_file_if_exists(path)
        .await?;
    Ok(match contents {
        Some(contents) => Ok(contents),
        // Gone between the check and the read.
        None => Err(PathCheck::Missing { at: path }),
    })
}

pub(super) async fn read_dir(
    ctx: &mut DiceComputations<'_>,
    path: IProjectRelativePath,
) -> buck2_error::Result<DirListing> {
    ctx.compute(&IoReadDirKey(path)).await?.dupe()
}

pub(super) async fn read_path_metadata_if_exists(
    ctx: &mut DiceComputations<'_>,
    path: IProjectRelativePath,
) -> buck2_error::Result<Option<RawPathMetadata<IProjectRelativePath>>> {
    ctx.compute(&IoPathMetadataKey(path)).await?.dupe()
}

/// Whether something is at the path, spelled exactly like that and not under an ignored prefix.
pub(super) async fn exists(
    ctx: &mut DiceComputations<'_>,
    path: IProjectRelativePath,
) -> buck2_error::Result<bool> {
    match check_path(ctx, path).await? {
        PathCheck::Present(_) => Ok(true),
        check @ PathCheck::ParentSymlink { .. } => {
            if !read_through_symlink(ctx, path, check, ReadKind::File).await? {
                return Ok(false);
            }
            let Some((parent, name)) = path.split_last() else {
                return Ok(true);
            };
            Ok(read_dir_cached(ctx, parent).await?.lookup(name).is_some())
        }
        _ => Ok(false),
    }
}

struct ReadDirCache(
    BuckDashMap<
        IProjectRelativePath,
        Arc<OnceCell<buck2_error::Result<PackedStringSlice<FileName, FileType>>>>,
    >,
);

pub fn initialize_read_dir_cache(data: &mut UserComputationData) {
    data.data.set(ReadDirCache(Default::default()));
}

#[derive(Allocative)]
pub struct FileChangeTracker {
    files_to_dirty: BuckMutSet<IoReadFileKey>,
    dirs_to_dirty: BuckMutSet<IoReadDirKey>,
    paths_to_dirty: BuckMutSet<IoPathMetadataKey>,
    checks_to_dirty: BuckMutSet<PathCheckKey>,

    maybe_modified_dirs: BuckMutSet<IProjectRelativePath>,
}

impl FileChangeTracker {
    pub fn new() -> Self {
        Self {
            files_to_dirty: Default::default(),
            dirs_to_dirty: Default::default(),
            paths_to_dirty: Default::default(),
            maybe_modified_dirs: Default::default(),
            checks_to_dirty: Default::default(),
        }
    }

    pub fn write_to_dice(mut self, ctx: &mut DiceTransactionUpdater) -> buck2_error::Result<()> {
        // See comment on `dir_entries_changed_for_watchman_bug`
        for p in self.paths_to_dirty.clone() {
            if let Some(dir) = p.0.parent() {
                if self.maybe_modified_dirs.contains(&dir) {
                    self.entry_added_or_removed(p.0);
                }
            }
        }

        ctx.changed(self.files_to_dirty)?;
        ctx.changed(self.dirs_to_dirty)?;
        ctx.changed(self.paths_to_dirty)?;
        ctx.changed(self.checks_to_dirty)?;

        Ok(())
    }

    fn entry_added_or_removed(&mut self, path: IProjectRelativePath) {
        self.paths_to_dirty.insert(IoPathMetadataKey(path));
        self.checks_to_dirty.insert(PathCheckKey(path));
        let parent = path.parent();
        if let Some(parent) = parent {
            // `None` only for the project root itself, which has no parent listing to dirty.
            self.insert_dir_keys(parent.to_owned());
        }
    }

    fn insert_dir_keys(&mut self, path: IProjectRelativePath) {
        self.dirs_to_dirty.insert(IoReadDirKey(path));
    }

    pub fn file_added_or_removed(&mut self, path: IProjectRelativePath) {
        self.file_contents_changed(path);
        self.entry_added_or_removed(path);
    }

    pub fn dir_added_or_removed(&mut self, path: IProjectRelativePath) {
        self.entry_added_or_removed(path);
    }

    /// Also covers the file having been replaced by something of another type, which watchers do
    /// not reliably distinguish from a modification.
    pub fn file_contents_changed(&mut self, path: IProjectRelativePath) {
        self.files_to_dirty.insert(IoReadFileKey(path));
        self.paths_to_dirty.insert(IoPathMetadataKey(path));
        self.checks_to_dirty.insert(PathCheckKey(path));
    }

    /// Normally, buck does not need the file watcher to tell it that a directory's entries have
    /// changed. However, in some cases file watcher want to force-invalidate directory listings,
    /// and so this exists. It should not normally be used.
    pub fn dir_entries_changed_force_invalidate(&mut self, path: IProjectRelativePath) {
        self.insert_dir_keys(path);
    }

    /// Normally, we ignore directory modification events from file watchers and instead compute
    /// them ourselves when a file in the directory is reported as having been added or removed.
    /// However, watchman has a bug in which it sometimes incorrectly doesn't report files as having
    /// been added/removed. We work around this by implementing some logic that marks a directory
    /// listing as being invalid if both the directory and at least one of its entries is reported
    /// as having been modified.
    ///
    /// We cannot unconditionally respect directory modification events from the file watcher, as it
    /// is not aware of our ignore rules.
    pub fn dir_entries_changed_for_watchman_bug(&mut self, path: IProjectRelativePath) {
        self.maybe_modified_dirs.insert(path);
    }
}
