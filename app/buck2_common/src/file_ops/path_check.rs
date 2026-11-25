/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_core::fs::i_project_rel_path::IProjectRelativePath;
use buck2_core::soft_error;
use buck2_error::BuckErrorContext;
use buck2_hash::BuckDashMap;
use derive_more::Display;
use dice::CancellationContext;
use dice::DiceComputations;
use dice::DiceData;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;

use crate::file_ops::baseline::HasFileSystemBaseline;
use crate::file_ops::io::read_dir_cached;
use crate::file_ops::metadata::FileType;
use crate::ignores::ignore_set::HasIgnoreSet;

/// What is at a project path, established component by component from the project root.
///
/// A path is present only if each of its components is an entry, spelled identically, of the
/// directory before it, so that the answer is the same on case insensitive file systems as on
/// case sensitive ones. Symlinks are leaves: nothing is found through one. An ignored path, and
/// everything under it, is not present. The project root itself is always a present directory.
#[derive(Copy, Clone, Dupe, Debug, PartialEq, Eq, Allocative, Pagable)]
pub enum PathCheck {
    Present(FileType),
    /// `at`, the path or a prefix of it, is not an entry of its parent.
    Missing {
        at: IProjectRelativePath,
    },
    /// `at`, the path or a prefix of it, matches the ignore set.
    Ignored {
        at: IProjectRelativePath,
    },
    /// `at`, a strict prefix of the path, is a symlink.
    ParentSymlink {
        at: IProjectRelativePath,
    },
    /// `at`, a strict prefix of the path, exists but is not a directory.
    ParentNotDirectory {
        at: IProjectRelativePath,
        file_type: FileType,
    },
}

pub async fn check_path(
    ctx: &mut DiceComputations<'_>,
    path: IProjectRelativePath,
) -> buck2_error::Result<PathCheck> {
    Ok(checked(ctx, path).await?.check())
}

async fn checked(
    ctx: &mut DiceComputations<'_>,
    path: IProjectRelativePath,
) -> buck2_error::Result<Checked> {
    ctx.compute(&PathCheckKey(path)).await?.dupe()
}

/// A [`PathCheck`] as DICE holds it: with the generation of what is at a present path, so that
/// the check of a path whose directory was replaced by another compares unequal to the one
/// before, and what was read at the path is read again.
///
/// Spelled out rather than a `PathCheck` beside a generation, which would be half as large again
/// for every path buck2 looks at.
#[derive(Copy, Clone, Dupe, Debug, PartialEq, Eq, Allocative, Pagable)]
pub(super) enum Checked {
    /// With the latest generation among the directories the path is in, and the path itself if
    /// it is a directory.
    Present(FileType, u64),
    Missing {
        at: IProjectRelativePath,
    },
    Ignored {
        at: IProjectRelativePath,
    },
    ParentSymlink {
        at: IProjectRelativePath,
    },
    ParentNotDirectory {
        at: IProjectRelativePath,
        file_type: FileType,
    },
}

impl Checked {
    fn check(self) -> PathCheck {
        match self {
            Checked::Present(file_type, _generation) => PathCheck::Present(file_type),
            Checked::Missing { at } => PathCheck::Missing { at },
            Checked::Ignored { at } => PathCheck::Ignored { at },
            Checked::ParentSymlink { at } => PathCheck::ParentSymlink { at },
            Checked::ParentNotDirectory { at, file_type } => {
                PathCheck::ParentNotDirectory { at, file_type }
            }
        }
    }
}

/// The generation of each path the file watcher has reported as a directory added or removed
/// with what is in it (`FileChangeTracker::dir_tree_added_or_removed`): a number that no path had
/// before, given at each report. A directory never reported has generation zero.
///
/// A directory reported as added may be another directory than the one last read at its path,
/// different all the way down: a directory renamed onto the path of another is one event, for the
/// directory, with none for the entries that came with it. The generation is what tells the two
/// apart. It goes into the check of the directory and of everything present below it, see
/// [`Checked`], so that what was read below the path is read again.
///
/// The generations are kept beside the graph rather than as keys in it: a key per directory would
/// be a node and an edge for each of the millions of directories that are never reported. They
/// are bounded by the paths the file watcher reports, which are interned and never freed anyway.
///
/// Checks read them without DICE tracking the read. What makes that sound is how they are
/// written: only by `FileChangeTracker::write_to_dice`, which dirties the check of the path in
/// the same call and is called after the change is on disk, and only with a number larger than
/// any given before in the process. So a check that has the new number, and what was computed
/// from it, was computed after the change, and the check of nothing present at or below the
/// path comes out as it was before. No value outlives the process whose numbers it has.
///
/// A transaction that started before the report can compute a check with the new number too.
/// What it then reads below the path is what is there after the change, because a directory is
/// listed only after its check is computed in the same transaction (see `read_dir_cached`): a
/// listing taken before the change came with a check that has the number from before.
///
/// A root that starts over shares these with the root it replaces, and is told of no change:
/// see [`DirectoryGenerations::start_over`].
#[derive(Default)]
pub struct DirectoryGenerations(BuckDashMap<IProjectRelativePath, u64>);

impl DirectoryGenerations {
    pub(super) fn added_or_removed(&self, dirs: impl IntoIterator<Item = IProjectRelativePath>) {
        let generation = Self::next();
        for dir in dirs {
            // Two reports at once get their numbers in one order and may write them in the other.
            self.0
                .entry(dir)
                .and_modify(|latest| *latest = generation.max(*latest))
                .or_insert(generation);
        }
    }

    /// Gives every reported directory another generation. For when the file watcher lost track
    /// of what changed and the daemon starts over on a fresh root: that root computes the check
    /// of a reported directory itself, and with the generation the old root had it would take
    /// what the old root read below the directory for what is there now.
    pub fn start_over(&self) {
        let generation = Self::next();
        for mut latest in self.0.iter_mut() {
            *latest = generation.max(*latest);
        }
    }

    /// Unique in the process, like a `FileSystemBaseline`: two projects that share a daemon, and
    /// what they read under equal baselines, must not take a directory replaced in one for the
    /// directory replaced in the other.
    fn next() -> u64 {
        static LATEST: AtomicU64 = AtomicU64::new(0);
        LATEST.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn of(&self, dir: IProjectRelativePath) -> u64 {
        self.0.get(&dir).map_or(0, |generation| *generation)
    }
}

pub trait HasDirectoryGenerations {
    fn get_directory_generations(&self) -> &DirectoryGenerations;
}

impl HasDirectoryGenerations for DiceData {
    fn get_directory_generations(&self) -> &DirectoryGenerations {
        self.get::<DirectoryGenerations>()
            .expect("directory generations should be set with the io provider")
    }
}

/// What a read wanted the path to be; decides the wording of a [`PathCheckError`].
#[derive(Copy, Clone, Dupe, Debug)]
pub enum ReadKind {
    Directory,
    File,
}

/// A read refused because of what the path check found.
#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
#[error("{}", self.describe())]
pub struct PathCheckError {
    path: IProjectRelativePath,
    check: PathCheck,
    /// The `project.ignore` pattern behind `PathCheck::Ignored`.
    pattern: Option<String>,
    kind: ReadKind,
}

impl PathCheckError {
    pub fn new(
        ctx: &DiceComputations<'_>,
        path: IProjectRelativePath,
        check: PathCheck,
        kind: ReadKind,
    ) -> Self {
        let pattern = match check {
            PathCheck::Ignored { at } => ctx
                .global_data()
                .get_ignore_set()
                .matches_candidate(&at)
                .map(ToOwned::to_owned),
            _ => None,
        };
        Self {
            path,
            check,
            pattern,
            kind,
        }
    }

    pub fn check(&self) -> PathCheck {
        self.check
    }

    fn describe(&self) -> String {
        let (noun, wanted) = match self.kind {
            ReadKind::Directory => ("Directory", FileType::Directory),
            ReadKind::File => ("File", FileType::File),
        };
        let path = self.path;
        let pattern = || {
            format!(
                "`project.ignore` contains `{}`",
                self.pattern.as_deref().unwrap_or("?")
            )
        };
        match self.check {
            PathCheck::Present(file_type) if file_type == wanted => {
                format!("{noun} `{path}` exists")
            }
            PathCheck::Present(FileType::Symlink) => {
                format!("`{path}` is a symlink, which buck2 does not follow")
            }
            PathCheck::Present(file_type) => format!(
                "`{path}` is a {}, not a {}",
                file_type.describe(),
                wanted.describe()
            ),
            PathCheck::Missing { at } if at == path => format!("{noun} `{path}` does not exist"),
            PathCheck::Missing { at } => {
                format!("{noun} `{path}` does not exist (`{at}` does not exist)")
            }
            PathCheck::Ignored { at } if at == path => {
                format!("{noun} `{path}` is ignored ({})", pattern())
            }
            PathCheck::Ignored { at } => {
                format!(
                    "{noun} `{path}` is ignored (`{at}` is ignored, {})",
                    pattern()
                )
            }
            PathCheck::ParentSymlink { at } => {
                format!("{noun} `{path}` is behind the symlink `{at}`, which buck2 does not follow")
            }
            PathCheck::ParentNotDirectory { at, file_type } => format!(
                "{noun} `{path}` does not exist (`{at}` is a {}, not a directory)",
                file_type.describe()
            ),
        }
    }
}

/// A read the check refuses, let through with a soft error until it becomes an error. The report
/// says why the read is refused, what reading it anyway costs, and how to stop.
#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
#[error("{}", self.describe())]
enum ToleratedRead {
    /// `at`, the path or a prefix of it, matches `pattern`.
    Ignored {
        path: IProjectRelativePath,
        at: IProjectRelativePath,
        pattern: Option<String>,
    },
}

impl ToleratedRead {
    fn describe(&self) -> String {
        match self {
            ToleratedRead::Ignored { path, at, pattern } => {
                let pattern = pattern.as_deref().unwrap_or("?");
                let subject = if path == at {
                    format!("`{path}` matches")
                } else {
                    format!("`{path}` is under `{at}`, which matches")
                };
                format!(
                    "{subject} the `project.ignore` pattern `{pattern}`. buck2 does not see \
                     changes to ignored files, so builds that read it can be stale, and reading \
                     ignored files will become an error. Stop reading it, or un-ignore it: narrow \
                     the pattern, or add a `!` pattern for the path"
                )
            }
        }
    }
}

/// What the check of `path` finds once `at`, the path or a prefix of it that the check found
/// ignored, is looked at anyway. The ignore set is not consulted below `at`, and nothing found
/// there depends on the file watcher, which reports no changes to ignored paths: only on the
/// check of the directory that `at` is in.
pub(super) async fn check_ignored_path(
    ctx: &mut DiceComputations<'_>,
    path: IProjectRelativePath,
    at: IProjectRelativePath,
) -> buck2_error::Result<PathCheck> {
    let mut below = path
        .strip_prefix(at)
        .internal_error("the check reported a prefix")?
        .iter();
    // The check of `path` is the same whatever directory `at` is in. That of the directory
    // tells one from another that was renamed onto its path, which the file watcher does report.
    if let Some(parent) = at.parent() {
        checked(ctx, parent.to_owned()).await?;
    }
    let mut prefix = at;
    loop {
        let file_type = match prefix.split_last() {
            Some((parent, name)) => match read_dir_cached(ctx, parent).await?.lookup(name) {
                Some(file_type) => file_type,
                None => return Ok(PathCheck::Missing { at: prefix }),
            },
            None => FileType::Directory,
        };
        let Some(name) = below.next() else {
            return Ok(PathCheck::Present(file_type));
        };
        match file_type {
            FileType::Directory => {}
            FileType::Symlink => return Ok(PathCheck::ParentSymlink { at: prefix }),
            file_type => {
                return Ok(PathCheck::ParentNotDirectory {
                    at: prefix,
                    file_type,
                });
            }
        }
        prefix = prefix.join(name.as_forward_rel_path());
    }
}

/// Reports a read of an ignored path and lets the caller go ahead with it; under `BUCK2_HARD_ERROR`
/// the report is the error, as for any soft error. Everything that calls this is a candidate for
/// refusal.
pub(super) fn read_ignored_path(
    ctx: &DiceComputations<'_>,
    path: IProjectRelativePath,
    at: IProjectRelativePath,
) -> buck2_error::Result<()> {
    let pattern = ctx
        .global_data()
        .get_ignore_set()
        .matches_candidate(&at)
        .map(ToOwned::to_owned);
    let sample_key = pattern.clone().unwrap_or_default();
    soft_error!(
        "path_check_ignored_read",
        ToleratedRead::Ignored { path, at, pattern }.into(),
        low_cardinality_key_for_additional_logview_samples: Some(Box::new(sample_key)),
    );
    Ok(())
}

/// Checks are invalidated by the file watcher through the `FileChangeTracker`, for exactly the
/// paths it reports. A change to a directory's entries reaches the checks of the paths under them
/// through the dependency of each check on its parent's, and so does a directory that the file
/// watcher reports as added with what is in it where there was one before: its check comes out
/// with another generation, and so does the check of everything present below it.
///
/// The listing of the parent is read through the per-transaction directory cache rather than
/// `IoReadDirKey`, so that checking a path records no dependency on its parent's listing. Were it
/// to, adding a file to a directory would invalidate every read under each of its siblings;
/// `tests/core/fs/invalidations/test_root_directory.rs` guards against this.
#[derive(
    Clone, Dupe, Copy, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable
)]
#[pagable_typetag(dice::DiceKeyDyn)]
pub(super) struct PathCheckKey(pub(super) IProjectRelativePath);

#[async_trait]
impl Key for PathCheckKey {
    type Value = buck2_error::Result<Checked>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        ctx.get_file_system_baseline().await?;
        let own_generation =
            |ctx: &DiceComputations| ctx.global_data().get_directory_generations().of(self.0);
        let Some((parent, name)) = self.0.split_last() else {
            return Ok(Checked::Present(FileType::Directory, own_generation(ctx)));
        };
        let parent_generation = match checked(ctx, parent).await? {
            Checked::Present(FileType::Directory, generation) => generation,
            Checked::Present(FileType::Symlink, _) => {
                return Ok(Checked::ParentSymlink { at: parent });
            }
            Checked::Present(file_type, _) => {
                return Ok(Checked::ParentNotDirectory {
                    at: parent,
                    file_type,
                });
            }
            inherited @ (Checked::Missing { .. }
            | Checked::Ignored { .. }
            | Checked::ParentSymlink { .. }
            | Checked::ParentNotDirectory { .. }) => return Ok(inherited),
        };
        if ctx.global_data().get_ignore_set().is_match(&self.0) {
            return Ok(Checked::Ignored { at: self.0 });
        }
        Ok(match read_dir_cached(ctx, parent).await?.lookup(name) {
            Some(FileType::Directory) => Checked::Present(
                FileType::Directory,
                parent_generation.max(own_generation(ctx)),
            ),
            Some(file_type) => Checked::Present(file_type, parent_generation),
            None => Checked::Missing { at: self.0 },
        })
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::Mutex;

    use assert_matches::assert_matches;
    use buck2_core::cells::CellResolver;
    use buck2_core::cells::cell_path::CellPath;
    use buck2_core::cells::cell_root_path::CellRootPathBuf;
    use buck2_core::cells::name::CellName;
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;
    use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
    use buck2_fs::fs_util;
    use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
    use dice::DetectCycles;
    use dice::Dice;
    use dice::DiceEvent;
    use dice::DiceEventListener;
    use dice::DiceTransaction;
    use dice::PagableValueSerialize;
    use dice::UserComputationData;

    use super::*;
    use crate::dice::cells::SetCellResolver;
    use crate::dice::data::testing::SetTestingIoProvider;
    use crate::file_ops::baseline::FileSystemBaseline;
    use crate::file_ops::baseline::SetFileSystemBaseline;
    use crate::file_ops::dice::DiceFileComputations;
    use crate::file_ops::io::FileChangeTracker;
    use crate::file_ops::io::initialize_read_dir_cache;
    use crate::file_ops::metadata::RawPathMetadata;
    use crate::file_ops::metadata::RawSymlink;
    use crate::ignores::file_ignores::FileIgnoreReason;
    use crate::ignores::ignore_set::IgnoreSet;
    use crate::ignores::ignore_set::SetIgnoreSet;
    use crate::io::DirectoryDoesNotExistSuggestion;
    use crate::io::ReadDirError;

    fn p(s: &str) -> IProjectRelativePath {
        IProjectRelativePath::intern_directly(ProjectRelativePath::new(s).unwrap())
    }

    struct Fixture {
        fs: ProjectRootTemp,
        dice: Arc<Dice>,
        root: dice::BranchId,
        baseline: FileSystemBaseline,
        cells: CellResolver,
    }

    impl Fixture {
        /// One cell, `root`, at the project root.
        async fn new(ignores: &str) -> buck2_error::Result<Self> {
            Self::with_cells(ignores, &[("root", "")]).await
        }

        /// The first cell is the root cell and gets the ignores.
        async fn with_cells(ignores: &str, cells: &[(&str, &str)]) -> buck2_error::Result<Self> {
            let fs = ProjectRootTemp::new()?;
            let root_cell = CellName::testing_new(cells[0].0);
            let cells = CellResolver::testing_with_names_and_paths(
                &cells
                    .iter()
                    .map(|(name, path)| {
                        (
                            CellName::testing_new(name),
                            CellRootPathBuf::new(ProjectRelativePathBuf::unchecked_new(
                                (*path).to_owned(),
                            )),
                        )
                    })
                    .collect::<Vec<_>>(),
            );
            let mut dice = Dice::builder();
            dice.set_testing_io_provider(&fs);
            dice.set_ignore_set(Arc::new(IgnoreSet::testing_new_single_cell(
                cells.dupe(),
                root_cell,
                ignores,
            )?));
            let dice = dice.build(DetectCycles::Enabled);
            let root = dice.new_root().await;
            Ok(Self {
                fs,
                dice,
                root,
                baseline: FileSystemBaseline::unique(),
                cells,
            })
        }

        /// A transaction that has had `changes` reported to it, all under one baseline, so that
        /// what one transaction read is reused by the next unless reported changed.
        async fn transaction(
            &self,
            changes: FileChangeTracker,
        ) -> buck2_error::Result<DiceTransaction> {
            let mut data = UserComputationData::new();
            initialize_read_dir_cache(&mut data);
            let mut updater = self.dice.updater_with_data(self.root, data);
            updater.set_cell_resolver(self.cells.dupe())?;
            updater.set_file_system_baseline(self.baseline.dupe())?;
            changes.write_to_dice(&mut updater)?;
            Ok(updater.commit().await)
        }

        /// What the daemon does when the file watcher lost track of what changed: the
        /// transactions that follow are on a fresh root, under the same baseline.
        async fn start_over(&mut self) {
            let updater = self.dice.updater(self.root);
            updater
                .global_data()
                .get_directory_generations()
                .start_over();
            self.root = self.dice.new_root_from(self.root).await;
        }

        /// Writes the target verbatim; `fs_util::symlink` makes it absolute on Windows, and
        /// the tests that look at the target want a relative one to stay relative.
        fn symlink(&self, link: &str, target: &str) {
            let link = self
                .fs
                .path()
                .resolve(ProjectRelativePath::new(link).unwrap());
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &link).unwrap();
            #[cfg(windows)]
            {
                // Windows symlinks are typed; a dangling one is a file link.
                let is_dir = link.as_path().parent().unwrap().join(target).is_dir();
                if is_dir {
                    std::os::windows::fs::symlink_dir(target, &link).unwrap();
                } else {
                    std::os::windows::fs::symlink_file(target, &link).unwrap();
                }
            }
        }
    }

    async fn check(ctx: &DiceTransaction, path: &str) -> PathCheck {
        check_path(&mut ctx.ctx(), p(path)).await.unwrap()
    }

    /// The entry names of a cell-level `read_dir`, or the error's message.
    async fn read_dir(ctx: &DiceTransaction, path: &str) -> Result<Vec<String>, String> {
        DiceFileComputations::read_dir(&mut ctx.ctx(), CellPath::testing_new(path))
            .await
            .map(|out| out.included.iter().map(|n| n.as_str().to_owned()).collect())
            .map_err(|e| format!("{e:#}"))
    }

    fn names(names: &[&str]) -> Result<Vec<String>, String> {
        Ok(names.iter().map(|n| (*n).to_owned()).collect())
    }

    fn fails_with(res: Result<Vec<String>, String>, needle: &str) {
        match res {
            Err(msg) => assert!(msg.contains(needle), "`{msg}` does not contain `{needle}`"),
            Ok(listing) => panic!("expected a failure mentioning `{needle}`, got {listing:?}"),
        }
    }

    #[tokio::test]
    async fn present_and_missing() -> buck2_error::Result<()> {
        let f = Fixture::new("").await?;
        f.fs.write_file("a/b/f", "");
        let ctx = f.transaction(FileChangeTracker::new()).await?;

        assert_eq!(
            check(&ctx, "").await,
            PathCheck::Present(FileType::Directory)
        );
        assert_eq!(
            check(&ctx, "a").await,
            PathCheck::Present(FileType::Directory)
        );
        assert_eq!(
            check(&ctx, "a/b").await,
            PathCheck::Present(FileType::Directory)
        );
        assert_eq!(
            check(&ctx, "a/b/f").await,
            PathCheck::Present(FileType::File)
        );
        assert_eq!(
            check(&ctx, "a/b/g").await,
            PathCheck::Missing { at: p("a/b/g") }
        );
        assert_eq!(
            check(&ctx, "a/x/y/z").await,
            PathCheck::Missing { at: p("a/x") }
        );
        assert_eq!(
            check(&ctx, "nope").await,
            PathCheck::Missing { at: p("nope") }
        );
        Ok(())
    }

    /// Spelling must match exactly, whatever the file system thinks.
    #[tokio::test]
    async fn case_must_match() -> buck2_error::Result<()> {
        let f = Fixture::new("").await?;
        f.fs.write_file("a/b/f", "");
        let ctx = f.transaction(FileChangeTracker::new()).await?;

        assert_eq!(
            check(&ctx, "a/B/f").await,
            PathCheck::Missing { at: p("a/B") }
        );
        assert_eq!(
            check(&ctx, "a/b/F").await,
            PathCheck::Missing { at: p("a/b/F") }
        );
        Ok(())
    }

    #[tokio::test]
    async fn symlinks_are_leaves() -> buck2_error::Result<()> {
        let f = Fixture::new("").await?;
        f.fs.write_file("a/b/f", "");
        f.symlink("l", "a");
        f.symlink("a/dangling", "nowhere");
        let ctx = f.transaction(FileChangeTracker::new()).await?;

        assert_eq!(
            check(&ctx, "l").await,
            PathCheck::Present(FileType::Symlink)
        );
        assert_eq!(
            check(&ctx, "l/b").await,
            PathCheck::ParentSymlink { at: p("l") }
        );
        assert_eq!(
            check(&ctx, "l/b/f").await,
            PathCheck::ParentSymlink { at: p("l") }
        );
        assert_eq!(
            check(&ctx, "a/dangling").await,
            PathCheck::Present(FileType::Symlink)
        );
        assert_eq!(
            check(&ctx, "a/dangling/x").await,
            PathCheck::ParentSymlink {
                at: p("a/dangling")
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn file_in_parent() -> buck2_error::Result<()> {
        let f = Fixture::new("").await?;
        f.fs.write_file("a/f", "");
        let ctx = f.transaction(FileChangeTracker::new()).await?;

        assert_eq!(
            check(&ctx, "a/f/x/y").await,
            PathCheck::ParentNotDirectory {
                at: p("a/f"),
                file_type: FileType::File
            }
        );
        Ok(())
    }

    /// Ignored paths are ignored whether or not they exist, and so is everything under them.
    #[tokio::test]
    async fn ignores() -> buck2_error::Result<()> {
        let f = Fixture::new("ign, **/*.pyc").await?;
        f.fs.write_file("ign/f", "");
        f.fs.write_file("a/x.pyc", "");
        let ctx = f.transaction(FileChangeTracker::new()).await?;

        assert_eq!(
            check(&ctx, "ign").await,
            PathCheck::Ignored { at: p("ign") }
        );
        assert_eq!(
            check(&ctx, "ign/f").await,
            PathCheck::Ignored { at: p("ign") }
        );
        assert_eq!(
            check(&ctx, "ign/g/h").await,
            PathCheck::Ignored { at: p("ign") }
        );
        assert_eq!(
            check(&ctx, "a").await,
            PathCheck::Present(FileType::Directory)
        );
        assert_eq!(
            check(&ctx, "a/x.pyc").await,
            PathCheck::Ignored { at: p("a/x.pyc") }
        );
        assert_eq!(
            check(&ctx, "a/x.pyc/y").await,
            PathCheck::Ignored { at: p("a/x.pyc") }
        );
        assert_eq!(
            check(&ctx, "buck-out/v2").await,
            PathCheck::Ignored { at: p("buck-out") }
        );
        Ok(())
    }

    /// A `!` pattern un-ignores what it matches and nothing above it: under a directory that
    /// stays ignored, so does everything.
    #[tokio::test]
    async fn negated_ignores() -> buck2_error::Result<()> {
        let f =
            Fixture::new("ign, **/*.gen, **/*.gen/**, !ign/kept, !a/*.gen, !a/*.gen/keep").await?;
        f.fs.write_file("ign/kept/f", "");
        f.fs.write_file("a/x.gen/keep", "");
        f.fs.write_file("a/x.gen/other", "");
        f.fs.write_file("b/y.gen/keep", "");
        let ctx = f.transaction(FileChangeTracker::new()).await?;

        assert_eq!(
            check(&ctx, "ign/kept").await,
            PathCheck::Ignored { at: p("ign") }
        );
        assert_eq!(
            check(&ctx, "ign/kept/f").await,
            PathCheck::Ignored { at: p("ign") }
        );
        assert_eq!(
            check(&ctx, "a/x.gen").await,
            PathCheck::Present(FileType::Directory)
        );
        assert_eq!(
            check(&ctx, "a/x.gen/keep").await,
            PathCheck::Present(FileType::File)
        );
        assert_eq!(
            check(&ctx, "a/x.gen/other").await,
            PathCheck::Ignored {
                at: p("a/x.gen/other")
            }
        );
        assert_eq!(
            check(&ctx, "b/y.gen/keep").await,
            PathCheck::Ignored { at: p("b/y.gen") }
        );
        assert_eq!(read_dir(&ctx, "root//").await, names(&["a", "b"]));
        assert_eq!(read_dir(&ctx, "root//a").await, names(&["x.gen"]));
        assert_eq!(read_dir(&ctx, "root//a/x.gen").await, names(&["keep"]));
        assert_eq!(read_dir(&ctx, "root//b").await, names(&[]));
        fails_with(
            read_dir(&ctx, "root//b/y.gen").await,
            "is ignored (`project.ignore` contains `**/*.gen`)",
        );
        Ok(())
    }

    /// A check is recomputed when the file watcher reports its path, and when it reports a
    /// prefix of it.
    #[tokio::test]
    async fn invalidation() -> buck2_error::Result<()> {
        let f = Fixture::new("").await?;
        f.fs.write_file("a/b/f", "");
        let ctx = f.transaction(FileChangeTracker::new()).await?;
        assert_eq!(
            check(&ctx, "a/b/f").await,
            PathCheck::Present(FileType::File)
        );
        assert_eq!(
            check(&ctx, "a/b/g").await,
            PathCheck::Missing { at: p("a/b/g") }
        );
        assert_eq!(check(&ctx, "c/d").await, PathCheck::Missing { at: p("c") });
        drop(ctx);

        fs_util::remove_file(
            f.fs.path()
                .resolve(ProjectRelativePath::new("a/b/f").unwrap()),
        )
        .unwrap();
        f.fs.write_file("a/b/g", "");
        f.fs.write_file("c/d", "");

        // Unreported changes are not seen.
        let ctx = f.transaction(FileChangeTracker::new()).await?;
        assert_eq!(
            check(&ctx, "a/b/f").await,
            PathCheck::Present(FileType::File)
        );
        assert_eq!(
            check(&ctx, "a/b/g").await,
            PathCheck::Missing { at: p("a/b/g") }
        );
        assert_eq!(check(&ctx, "c/d").await, PathCheck::Missing { at: p("c") });
        drop(ctx);

        let mut changes = FileChangeTracker::new();
        changes.file_added_or_removed(p("a/b/f"));
        changes.file_added_or_removed(p("a/b/g"));
        changes.dir_added_or_removed(p("c"));
        let ctx = f.transaction(changes).await?;
        assert_eq!(
            check(&ctx, "a/b/f").await,
            PathCheck::Missing { at: p("a/b/f") }
        );
        assert_eq!(
            check(&ctx, "a/b/g").await,
            PathCheck::Present(FileType::File)
        );
        assert_eq!(check(&ctx, "c/d").await, PathCheck::Present(FileType::File));
        Ok(())
    }

    /// Listing a directory goes through the check, and leaves out ignored entries.
    #[tokio::test]
    async fn read_dir_through_the_check() -> buck2_error::Result<()> {
        let f = Fixture::new("ign, **/*.pyc").await?;
        f.fs.write_file("a/b/f", "");
        f.fs.write_file("a/x.pyc", "");
        f.fs.write_file("ign/f", "");
        f.symlink("l", "a");
        let ctx = f.transaction(FileChangeTracker::new()).await?;

        assert_eq!(read_dir(&ctx, "root//a").await, names(&["b"]));
        assert_eq!(read_dir(&ctx, "root//").await, names(&["a", "l"]));
        fails_with(
            read_dir(&ctx, "root//ign").await,
            "is ignored (`project.ignore` contains `ign`)",
        );
        fails_with(
            read_dir(&ctx, "root//ign/g").await,
            "(`ign` is ignored, `project.ignore` contains `ign`)",
        );
        fails_with(
            read_dir(&ctx, "root//l").await,
            "is a symlink, which buck2 does not follow",
        );
        fails_with(
            read_dir(&ctx, "root//l/b").await,
            "is behind the symlink `l`",
        );
        fails_with(
            read_dir(&ctx, "root//a/b/f").await,
            "is a file, not a directory",
        );
        fails_with(
            read_dir(&ctx, "root//a/b/f/g").await,
            "(`a/b/f` is a file, not a directory)",
        );
        fails_with(
            read_dir(&ctx, "root//nope").await,
            "Directory `nope` does not exist",
        );
        fails_with(
            read_dir(&ctx, "root//a/B").await,
            "Directory `a/B` does not exist",
        );
        fails_with(
            read_dir(&ctx, "root//a/B/c").await,
            "(`a/B` does not exist)",
        );
        Ok(())
    }

    /// `read_dir_unless_behind_symlink` finds nothing at a symlink or behind one, and is `read_dir`
    /// everywhere else.
    #[tokio::test]
    async fn read_dir_unless_behind_symlink() -> buck2_error::Result<()> {
        let f = Fixture::new("ign").await?;
        f.fs.write_file("a/b/f", "");
        f.fs.write_file("ign/f", "");
        f.symlink("l", "a");
        let outside = ProjectRootTemp::new()?;
        outside.write_file("b/f", "");
        f.symlink("e", outside.path().root().as_path().to_str().unwrap());

        let list = async |ctx: &DiceTransaction, path: &str| {
            DiceFileComputations::read_dir_unless_behind_symlink(&mut ctx.ctx(), cp(path))
                .await
                .map(|out| {
                    out.map(|out| {
                        out.included
                            .iter()
                            .map(|n| n.as_str().to_owned())
                            .collect::<Vec<_>>()
                    })
                })
                .map_err(|e| format!("{e:#}"))
        };
        let ctx = f.transaction(FileChangeTracker::new()).await?;
        assert_eq!(list(&ctx, "root//a").await, Ok(Some(vec!["b".to_owned()])));
        for behind in ["root//l", "root//l/b", "root//e", "root//e/b"] {
            assert_eq!(list(&ctx, behind).await, Ok(None), "{behind}");
        }
        assert!(
            list(&ctx, "root//ign")
                .await
                .unwrap_err()
                .contains("is ignored")
        );
        assert!(
            list(&ctx, "root//nope")
                .await
                .unwrap_err()
                .contains("does not exist")
        );
        Ok(())
    }

    /// Nested cell roots are not entries of the enclosing cell, and not reachable through it.
    #[tokio::test]
    async fn read_dir_and_nested_cells() -> buck2_error::Result<()> {
        let f = Fixture::with_cells("", &[("root", ""), ("sub", "sub")]).await?;
        f.fs.write_file("sub/f", "");
        f.fs.write_file("x/f", "");
        let ctx = f.transaction(FileChangeTracker::new()).await?;

        assert_eq!(read_dir(&ctx, "root//").await, names(&["x"]));
        assert_eq!(read_dir(&ctx, "sub//").await, names(&["f"]));
        fails_with(
            read_dir(&ctx, "root//sub").await,
            "file is part of cell `sub`",
        );
        fails_with(
            read_dir(&ctx, "root//sub/f").await,
            "file is part of cell `sub`",
        );
        Ok(())
    }

    #[tokio::test]
    async fn exists() -> buck2_error::Result<()> {
        let f = Fixture::with_cells("ign", &[("root", ""), ("sub", "sub")]).await?;
        f.fs.write_file("a/b/f", "");
        f.fs.write_file("ign/f", "");
        f.fs.write_file("sub/f", "");
        f.symlink("l", "a");
        let ctx = f.transaction(FileChangeTracker::new()).await?;

        let exists = async |path: &str| {
            DiceFileComputations::exists(&mut ctx.ctx(), CellPath::testing_new(path))
                .await
                .unwrap()
        };
        assert!(exists("root//a/b/f").await);
        assert!(exists("root//a/b").await);
        assert!(exists("root//l").await);
        assert!(exists("sub//f").await);
        assert!(!exists("root//a/B/f").await);
        assert!(!exists("root//a/b/F").await);
        assert!(!exists("root//l/b").await);
        assert!(!exists("root//ign/f").await);
        assert!(!exists("root//missing/f").await);
        assert!(!exists("root//sub/f").await);
        Ok(())
    }

    fn cp(s: &str) -> CellPath {
        CellPath::testing_new(s)
    }

    async fn metadata(ctx: &DiceTransaction, path: &str) -> Option<RawPathMetadata> {
        DiceFileComputations::read_path_metadata_if_exists(&mut ctx.ctx(), cp(path))
            .await
            .unwrap()
    }

    /// Metadata goes through the check; a symlink above the path is reported as the symlink,
    /// with the remainder of the path appended to its target.
    #[tokio::test]
    async fn metadata_through_the_check() -> buck2_error::Result<()> {
        let f = Fixture::new("ign").await?;
        f.fs.write_file("a/b/f", "");
        f.fs.write_file("ign/f", "");
        f.symlink("l", "a");
        // Absolute on the host, so that it is external on Windows too.
        let elsewhere = if cfg!(windows) {
            "C:/nowhere/else"
        } else {
            "/nowhere/else"
        };
        f.symlink("e", elsewhere);
        let ctx = f.transaction(FileChangeTracker::new()).await?;

        assert_eq!(
            metadata(&ctx, "root//").await,
            Some(RawPathMetadata::Directory)
        );
        assert_eq!(
            metadata(&ctx, "root//a").await,
            Some(RawPathMetadata::Directory)
        );
        assert_matches!(
            metadata(&ctx, "root//a/b/f").await,
            Some(RawPathMetadata::File(_))
        );
        assert_eq!(metadata(&ctx, "root//a/B/f").await, None);
        assert_eq!(metadata(&ctx, "root//a/b/f/x").await, None);
        assert_eq!(metadata(&ctx, "root//nope").await, None);

        assert_matches!(
            metadata(&ctx, "root//l").await,
            Some(RawPathMetadata::Symlink { at, to: RawSymlink::Relative(target, raw) }) => {
                assert_eq!(at, cp("root//l"));
                assert_eq!(target, cp("root//a"));
                assert_eq!(raw.target().as_str(), "a");
            }
        );
        assert_matches!(
            metadata(&ctx, "root//l/b/f").await,
            Some(RawPathMetadata::Symlink { at, to: RawSymlink::Relative(target, raw) }) => {
                assert_eq!(at, cp("root//l"));
                assert_eq!(target, cp("root//a/b/f"));
                assert_eq!(raw.target().as_str(), "a/b/f");
            }
        );
        assert_matches!(
            metadata(&ctx, "root//e/x/y").await,
            Some(RawPathMetadata::Symlink { at, to: RawSymlink::External(external) }) => {
                assert_eq!(at, cp("root//e"));
                assert_eq!(external.target().to_str().unwrap(), elsewhere);
                assert_eq!(external.remaining_path().as_str(), "x/y");
            }
        );
        Ok(())
    }

    /// The warning leads with why the read is refused: the pattern, and for a path under an
    /// ignored directory, which directory.
    #[test]
    fn tolerated_read_ignored_says_why() {
        let pattern = Some("**/*.orig".to_owned());
        assert_eq!(
            ToleratedRead::Ignored {
                path: p("a/f.orig"),
                at: p("a/f.orig"),
                pattern: pattern.clone(),
            }
            .to_string(),
            "`a/f.orig` matches the `project.ignore` pattern `**/*.orig`. buck2 does not see \
             changes to ignored files, so builds that read it can be stale, and reading ignored \
             files will become an error. Stop reading it, or un-ignore it: narrow the pattern, \
             or add a `!` pattern for the path"
        );
        assert!(
            ToleratedRead::Ignored {
                path: p("a/x.orig/f"),
                at: p("a/x.orig"),
                pattern,
            }
            .to_string()
            .starts_with(
                "`a/x.orig/f` is under `a/x.orig`, which matches the `project.ignore` pattern \
                 `**/*.orig`."
            )
        );
    }

    /// Ignored paths are read anyway, and what is below the ignored prefix is checked like any
    /// other path.
    #[tokio::test]
    async fn metadata_of_ignored_paths() -> buck2_error::Result<()> {
        let f = Fixture::new("ign,**/*.orig").await?;
        f.fs.write_file("ign/d/f", "");
        f.fs.write_file("a/f.orig", "");
        f.symlink("ign/l", "d");
        let ctx = f.transaction(FileChangeTracker::new()).await?;

        assert_matches!(
            metadata(&ctx, "root//a/f.orig").await,
            Some(RawPathMetadata::File(_))
        );
        assert_eq!(
            metadata(&ctx, "root//ign").await,
            Some(RawPathMetadata::Directory)
        );
        assert_matches!(
            metadata(&ctx, "root//ign/d/f").await,
            Some(RawPathMetadata::File(_))
        );
        assert_matches!(
            metadata(&ctx, "root//ign/l/f").await,
            Some(RawPathMetadata::Symlink { at, to: RawSymlink::Relative(target, _) }) => {
                assert_eq!(at, cp("root//ign/l"));
                assert_eq!(target, cp("root//ign/d/f"));
            }
        );
        assert_eq!(metadata(&ctx, "root//a/g.orig").await, None);
        assert_eq!(metadata(&ctx, "root//ign/d/g").await, None);
        assert_eq!(metadata(&ctx, "root//ign/D/f").await, None);
        assert_eq!(metadata(&ctx, "root//ign/d/f/x").await, None);
        Ok(())
    }

    /// Metadata behind a symlink follows the symlink when the watcher reports it changed.
    #[tokio::test]
    async fn metadata_behind_a_changed_symlink() -> buck2_error::Result<()> {
        let f = Fixture::new("").await?;
        f.fs.write_file("a/f", "");
        f.fs.write_file("a2/f", "");
        f.symlink("l", "a");
        let ctx = f.transaction(FileChangeTracker::new()).await?;
        assert_matches!(
            metadata(&ctx, "root//l/f").await,
            Some(RawPathMetadata::Symlink { to: RawSymlink::Relative(target, _), .. }) => {
                assert_eq!(target, cp("root//a/f"));
            }
        );
        drop(ctx);

        fs_util::remove_file(f.fs.path().resolve(ProjectRelativePath::new("l").unwrap())).unwrap();
        f.symlink("l", "a2");
        let mut changes = FileChangeTracker::new();
        changes.file_contents_changed(p("l"));
        let ctx = f.transaction(changes).await?;
        assert_matches!(
            metadata(&ctx, "root//l/f").await,
            Some(RawPathMetadata::Symlink { to: RawSymlink::Relative(target, _), .. }) => {
                assert_eq!(target, cp("root//a2/f"));
            }
        );
        Ok(())
    }

    /// File reads go through the check: absent and ignored files read as `None`, symlinks and
    /// directories are errors.
    #[tokio::test]
    async fn read_file_through_the_check() -> buck2_error::Result<()> {
        let f = Fixture::new("ign").await?;
        f.fs.write_file("a/f", "contents");
        f.fs.write_file("ign/f", "");
        f.symlink("l", "a");
        f.symlink("a/lf", "f");
        let ctx = f.transaction(FileChangeTracker::new()).await?;

        let read = async |path: &str| {
            DiceFileComputations::read_file_if_exists(&mut ctx.ctx(), cp(path))
                .await
                .map_err(|e| format!("{e:#}"))
        };
        assert_eq!(read("root//a/f").await, Ok(Some("contents".to_owned())));
        assert_eq!(read("root//a/g").await, Ok(None));
        assert_eq!(read("root//a/F").await, Ok(None));
        assert_eq!(read("root//ign/f").await, Ok(None));
        assert_eq!(read("root//a/f/x").await, Ok(None));
        let fails = |res: Result<Option<String>, String>, needle: &str| match res {
            Err(msg) => assert!(msg.contains(needle), "`{msg}` does not contain `{needle}`"),
            Ok(v) => panic!("expected a failure mentioning `{needle}`, got {v:?}"),
        };
        fails(
            read("root//l/f").await,
            "File `l/f` is behind the symlink `l`",
        );
        fails(
            read("root//a/lf").await,
            "`a/lf` is a symlink, which buck2 does not follow",
        );
        fails(read("root//a").await, "`a` is a directory, not a file");
        Ok(())
    }

    async fn read_dir_error(ctx: &DiceTransaction, path: &str) -> ReadDirError {
        match DiceFileComputations::read_dir_ext(&mut ctx.ctx(), cp(path)).await {
            Err(e) => e,
            Ok(listing) => panic!("expected `{path}` to be refused, got {listing:?}"),
        }
    }

    /// `read_dir_ext` says which component is wrong and what might have been meant.
    #[tokio::test]
    async fn read_dir_errors_with_suggestions() -> buck2_error::Result<()> {
        let f = Fixture::with_cells("ign", &[("root", ""), ("sub", "sub")]).await?;
        f.fs.write_file("a/b/f", "");
        f.fs.write_file("ign/f", "");
        f.fs.write_file("sub/shared/f", "");
        f.fs.write_file("x", "");
        f.symlink("l", "a");
        let ctx = f.transaction(FileChangeTracker::new()).await?;

        assert_matches!(
            read_dir_error(&ctx, "root//a/B/c").await,
            ReadDirError::DirectoryDoesNotExist { path, suggestion: DirectoryDoesNotExistSuggestion::Typo(s) } => {
                assert_eq!(path, cp("root//a/B"));
                assert_eq!(s, "b");
            }
        );
        assert_matches!(
            read_dir_error(&ctx, "root//shared").await,
            ReadDirError::DirectoryDoesNotExist { path, suggestion: DirectoryDoesNotExistSuggestion::Cell(cells) } => {
                assert_eq!(path, cp("root//shared"));
                assert_eq!(cells, vec!["sub//shared".to_owned()]);
            }
        );
        assert_matches!(
            read_dir_error(&ctx, "root//nothing/like/it").await,
            ReadDirError::DirectoryDoesNotExist { path, suggestion: DirectoryDoesNotExistSuggestion::NoSuggestion } => {
                assert_eq!(path, cp("root//nothing"));
            }
        );
        assert_matches!(
            read_dir_error(&ctx, "root//ign/f").await,
            ReadDirError::DirectoryIsIgnored(path, FileIgnoreReason::IgnoredByPattern { pattern, .. }) => {
                assert_eq!(path, cp("root//ign"));
                assert_eq!(pattern, "ign");
            }
        );
        assert_matches!(
            read_dir_error(&ctx, "root//sub/shared").await,
            ReadDirError::DirectoryIsIgnored(path, FileIgnoreReason::IgnoredByCell { cell_name, .. }) => {
                assert_eq!(path, cp("root//sub"));
                assert_eq!(cell_name, CellName::testing_new("sub"));
            }
        );
        assert_matches!(
            read_dir_error(&ctx, "root//l/b").await,
            ReadDirError::Symlink { path, at } => {
                assert_eq!(path, cp("root//l/b"));
                assert_eq!(at, cp("root//l"));
            }
        );
        assert_matches!(
            read_dir_error(&ctx, "root//l").await,
            ReadDirError::Symlink { path, at } => {
                assert_eq!(path, cp("root//l"));
                assert_eq!(at, cp("root//l"));
            }
        );
        assert_matches!(
            read_dir_error(&ctx, "root//x/y").await,
            ReadDirError::NotADirectory(path, kind) => {
                assert_eq!(path, cp("root//x"));
                assert_eq!(kind, "file");
            }
        );
        Ok(())
    }

    /// `read_file` explains what it found instead of a file.
    #[tokio::test]
    async fn read_file_errors() -> buck2_error::Result<()> {
        let f = Fixture::new("ign").await?;
        f.fs.write_file("a/f", "");
        f.fs.write_file("ign/f", "");
        let ctx = f.transaction(FileChangeTracker::new()).await?;

        let read = async |path: &str| {
            DiceFileComputations::read_file(&mut ctx.ctx(), cp(path))
                .await
                .map_err(|e| format!("{:#}", e.without_package_context_information()))
        };
        assert_eq!(read("root//a/f").await, Ok(String::new()));
        assert_eq!(
            read("root//a/g").await,
            Err("File `a/g` does not exist".to_owned())
        );
        assert_eq!(
            read("root//b/g").await,
            Err("File `b/g` does not exist (`b` does not exist)".to_owned())
        );
        assert_eq!(
            read("root//ign/f").await,
            Err(
                "File `ign/f` is ignored (`ign` is ignored, `project.ignore` contains `ign`)"
                    .to_owned()
            )
        );
        assert_eq!(
            read("root//a").await,
            Err("`a` is a directory, not a file".to_owned())
        );
        Ok(())
    }

    #[test]
    fn a_check_with_a_generation_is_no_larger_than_one_without() {
        assert_eq!(
            size_of::<buck2_error::Result<Checked>>(),
            size_of::<buck2_error::Result<PathCheck>>()
        );
    }

    fn resolve(f: &Fixture, path: &str) -> AbsNormPathBuf {
        f.fs.path().resolve(ProjectRelativePath::new(path).unwrap())
    }

    fn rename(f: &Fixture, from: &str, to: &str) {
        std::fs::rename(resolve(f, from), resolve(f, to)).unwrap();
    }

    /// What a file watcher that names only the directory reports for its rename, as the EdenFS
    /// one does.
    fn renamed(changes: &mut FileChangeTracker, from: &str, to: &str) {
        changes.dir_tree_added_or_removed(p(to));
        changes.dir_tree_added_or_removed(p(from));
    }

    async fn read_file(ctx: &DiceTransaction, path: &str) -> Option<String> {
        DiceFileComputations::read_file_if_exists(&mut ctx.ctx(), cp(path))
            .await
            .unwrap()
    }

    /// What a key that read the file has of it. `read_file` reads the file each time it is
    /// asked, and so does not tell whether DICE has what depends on the file computed again.
    async fn contents(ctx: &DiceTransaction, path: &str) -> String {
        ctx.ctx().compute(&Contents(p(path))).await.unwrap().clone()
    }

    /// A directory `d` that has been read throughout, then replaced by the directory `incoming`,
    /// with nothing reported but the two renames.
    async fn replaced_directory() -> buck2_error::Result<(Fixture, DiceTransaction)> {
        let f = Fixture::new("").await?;
        f.fs.write_file("d/old", "");
        f.fs.write_file("d/same", "old");
        f.fs.write_file("d/gone", "gone");
        f.fs.write_file("d/sub/x", "");
        f.fs.write_file("d/was_file", "");
        f.fs.write_file("incoming/new", "");
        f.fs.write_file("incoming/same", "new");
        f.fs.write_file("incoming/appeared", "appeared");
        f.fs.write_file("incoming/sub/y", "");
        f.fs.write_file("incoming/was_file/in", "");
        f.fs.write_file("incoming/was_missing/in", "");
        let ctx = f.transaction(FileChangeTracker::new()).await?;
        assert_eq!(
            read_dir(&ctx, "root//d").await,
            names(&["gone", "old", "same", "sub", "was_file"])
        );
        assert_eq!(read_dir(&ctx, "root//d/sub").await, names(&["x"]));
        assert_eq!(contents(&ctx, "d/same").await, "old");
        assert_eq!(
            read_file(&ctx, "root//d/gone").await.as_deref(),
            Some("gone")
        );
        assert_eq!(read_file(&ctx, "root//d/appeared").await, None);
        assert_matches!(
            metadata(&ctx, "root//d/same").await,
            Some(RawPathMetadata::File(_))
        );
        assert_eq!(
            check(&ctx, "d/was_file").await,
            PathCheck::Present(FileType::File)
        );
        fails_with(
            read_dir(&ctx, "root//d/was_missing").await,
            "Directory `d/was_missing` does not exist",
        );
        drop(ctx);

        rename(&f, "d", "old");
        rename(&f, "incoming", "d");
        let mut changes = FileChangeTracker::new();
        renamed(&mut changes, "d", "old");
        renamed(&mut changes, "incoming", "d");
        let ctx = f.transaction(changes).await?;
        Ok((f, ctx))
    }

    /// A directory renamed onto the path of another is reported as a directory that was added:
    /// nothing is reported for what is in it, and the path checks out as it did before, a
    /// directory. Its listing is read again all the same.
    #[tokio::test]
    async fn a_directory_replaced_by_another_is_listed_again() -> buck2_error::Result<()> {
        let (_f, ctx) = replaced_directory().await?;
        assert_eq!(
            read_dir(&ctx, "root//d").await,
            names(&["appeared", "new", "same", "sub", "was_file", "was_missing"])
        );
        assert_eq!(
            read_dir(&ctx, "root//old").await,
            names(&["gone", "old", "same", "sub", "was_file"])
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_directory_below_a_replaced_one_is_listed_again() -> buck2_error::Result<()> {
        let (_f, ctx) = replaced_directory().await?;
        assert_eq!(read_dir(&ctx, "root//d/sub").await, names(&["y"]));
        Ok(())
    }

    #[tokio::test]
    async fn an_entry_that_left_with_a_replaced_directory_is_gone() -> buck2_error::Result<()> {
        let (_f, ctx) = replaced_directory().await?;
        assert_eq!(
            check(&ctx, "d/gone").await,
            PathCheck::Missing { at: p("d/gone") }
        );
        assert_eq!(read_file(&ctx, "root//d/gone").await, None);
        assert_eq!(metadata(&ctx, "root//d/gone").await, None);
        Ok(())
    }

    #[tokio::test]
    async fn a_file_of_the_same_name_in_a_replaced_directory_is_read_again()
    -> buck2_error::Result<()> {
        let f = Fixture::new("").await?;
        f.fs.write_file("d/same", "old");
        f.fs.write_file("incoming/same", "new, and longer");
        let ctx = f.transaction(FileChangeTracker::new()).await?;
        assert_eq!(contents(&ctx, "d/same").await, "old");
        let before = metadata(&ctx, "root//d/same").await;
        drop(ctx);

        rename(&f, "d", "old");
        rename(&f, "incoming", "d");
        let mut changes = FileChangeTracker::new();
        renamed(&mut changes, "d", "old");
        renamed(&mut changes, "incoming", "d");
        let ctx = f.transaction(changes).await?;
        assert_eq!(contents(&ctx, "d/same").await, "new, and longer");
        let after = metadata(&ctx, "root//d/same").await;
        assert_matches!(after, Some(RawPathMetadata::File(_)));
        assert_ne!(after, before);
        Ok(())
    }

    #[tokio::test]
    async fn an_entry_that_came_with_a_replaced_directory_is_found() -> buck2_error::Result<()> {
        let (_f, ctx) = replaced_directory().await?;
        assert_eq!(
            read_file(&ctx, "root//d/appeared").await.as_deref(),
            Some("appeared")
        );
        Ok(())
    }

    /// An ignored file is read with a warning, and what was read is kept: the file watcher
    /// reports no changes to it. A replaced directory above it is reported.
    #[tokio::test]
    async fn an_ignored_file_in_a_replaced_directory_is_read_again() -> buck2_error::Result<()> {
        let f = Fixture::new("**/*.pyc").await?;
        f.fs.write_file("d/x.pyc", "old");
        f.fs.write_file("incoming/x.pyc", "new, and longer");
        let ctx = f.transaction(FileChangeTracker::new()).await?;
        let before = metadata(&ctx, "root//d/x.pyc").await;
        assert_matches!(before, Some(RawPathMetadata::File(_)));
        drop(ctx);

        rename(&f, "d", "old");
        rename(&f, "incoming", "d");
        let mut changes = FileChangeTracker::new();
        renamed(&mut changes, "d", "old");
        renamed(&mut changes, "incoming", "d");
        let ctx = f.transaction(changes).await?;
        let after = metadata(&ctx, "root//d/x.pyc").await;
        assert_matches!(after, Some(RawPathMetadata::File(_)));
        assert_ne!(after, before);
        Ok(())
    }

    /// What was a file, or missing, in the directory that was there before is listed as the
    /// directory it is now.
    #[tokio::test]
    async fn a_directory_that_came_with_a_replaced_directory_is_listed() -> buck2_error::Result<()>
    {
        let (_f, ctx) = replaced_directory().await?;
        assert_eq!(read_dir(&ctx, "root//d/was_file").await, names(&["in"]));
        assert_eq!(read_dir(&ctx, "root//d/was_missing").await, names(&["in"]));
        Ok(())
    }

    /// POSIX renames a directory onto an empty one, and EdenFS reports it as it does any other
    /// rename of a directory.
    #[tokio::test]
    async fn a_directory_renamed_onto_an_empty_one_is_read_again() -> buck2_error::Result<()> {
        let f = Fixture::new("").await?;
        fs_util::create_dir_all(resolve(&f, "d")).unwrap();
        f.fs.write_file("incoming/appeared", "appeared");
        let ctx = f.transaction(FileChangeTracker::new()).await?;
        assert_eq!(read_dir(&ctx, "root//d").await, names(&[]));
        assert_eq!(read_file(&ctx, "root//d/appeared").await, None);
        drop(ctx);

        rename(&f, "incoming", "d");
        let mut changes = FileChangeTracker::new();
        renamed(&mut changes, "incoming", "d");
        let ctx = f.transaction(changes).await?;
        assert_eq!(read_dir(&ctx, "root//d").await, names(&["appeared"]));
        assert_eq!(
            read_file(&ctx, "root//d/appeared").await.as_deref(),
            Some("appeared")
        );
        Ok(())
    }

    #[tokio::test]
    async fn swapped_directories_are_read_again() -> buck2_error::Result<()> {
        let f = Fixture::new("").await?;
        f.fs.write_file("a/f", "a");
        f.fs.write_file("a/only_a", "");
        f.fs.write_file("b/f", "b");
        f.fs.write_file("b/only_b", "");
        let ctx = f.transaction(FileChangeTracker::new()).await?;
        assert_eq!(read_dir(&ctx, "root//a").await, names(&["f", "only_a"]));
        assert_eq!(read_dir(&ctx, "root//b").await, names(&["f", "only_b"]));
        assert_eq!(contents(&ctx, "a/f").await, "a");
        assert_eq!(contents(&ctx, "b/f").await, "b");
        drop(ctx);

        let mut changes = FileChangeTracker::new();
        for (from, to) in [("a", "tmp"), ("b", "a"), ("tmp", "b")] {
            rename(&f, from, to);
            renamed(&mut changes, from, to);
        }
        let ctx = f.transaction(changes).await?;
        assert_eq!(read_dir(&ctx, "root//a").await, names(&["f", "only_b"]));
        assert_eq!(read_dir(&ctx, "root//b").await, names(&["f", "only_a"]));
        assert_eq!(contents(&ctx, "a/f").await, "b");
        assert_eq!(contents(&ctx, "b/f").await, "a");
        Ok(())
    }

    /// A change to a file while its directory has another name is reported under that name, and
    /// is found under the one the directory ends up with.
    #[tokio::test]
    async fn a_directory_renamed_away_and_back_is_read_again() -> buck2_error::Result<()> {
        let f = Fixture::new("").await?;
        f.fs.write_file("d/f", "before");
        let ctx = f.transaction(FileChangeTracker::new()).await?;
        assert_eq!(contents(&ctx, "d/f").await, "before");
        assert_eq!(read_file(&ctx, "root//d/g").await, None);
        drop(ctx);

        let mut changes = FileChangeTracker::new();
        rename(&f, "d", "elsewhere");
        renamed(&mut changes, "d", "elsewhere");
        f.fs.write_file("elsewhere/f", "after");
        changes.file_contents_changed(p("elsewhere/f"));
        f.fs.write_file("elsewhere/g", "new");
        changes.file_added_or_removed(p("elsewhere/g"));
        rename(&f, "elsewhere", "d");
        renamed(&mut changes, "elsewhere", "d");
        let ctx = f.transaction(changes).await?;
        assert_eq!(contents(&ctx, "d/f").await, "after");
        assert_eq!(read_file(&ctx, "root//d/g").await.as_deref(), Some("new"));
        Ok(())
    }

    /// A replaced directory that no command reads is still read again by the first that does.
    #[tokio::test]
    async fn a_replaced_directory_is_read_again_by_a_later_transaction() -> buck2_error::Result<()>
    {
        let (f, ctx) = replaced_directory().await?;
        drop(ctx);
        let ctx = f.transaction(FileChangeTracker::new()).await?;
        assert_eq!(read_dir(&ctx, "root//d/sub").await, names(&["y"]));
        assert_eq!(contents(&ctx, "d/same").await, "new");
        Ok(())
    }

    /// Each replacement is told from the one before it.
    #[tokio::test]
    async fn a_directory_replaced_twice_is_read_again_each_time() -> buck2_error::Result<()> {
        let f = Fixture::new("").await?;
        f.fs.write_file("d/f", "first");
        f.fs.write_file("second/f", "second");
        f.fs.write_file("third/f", "third");
        let ctx = f.transaction(FileChangeTracker::new()).await?;
        assert_eq!(contents(&ctx, "d/f").await, "first");
        drop(ctx);

        for (incoming, kept) in [("second", "kept_first"), ("third", "kept_second")] {
            rename(&f, "d", kept);
            rename(&f, incoming, "d");
            let mut changes = FileChangeTracker::new();
            renamed(&mut changes, "d", kept);
            renamed(&mut changes, incoming, "d");
            let ctx = f.transaction(changes).await?;
            assert_eq!(contents(&ctx, "d/f").await, incoming);
        }
        Ok(())
    }

    /// A directory replaced after one below it was: what is below both is read again.
    #[tokio::test]
    async fn a_directory_above_a_replaced_one_is_read_again_when_replaced()
    -> buck2_error::Result<()> {
        let f = Fixture::new("").await?;
        f.fs.write_file("d/sub/f", "first");
        f.fs.write_file("incoming_sub/f", "second");
        f.fs.write_file("incoming/sub/f", "third");
        let ctx = f.transaction(FileChangeTracker::new()).await?;
        assert_eq!(contents(&ctx, "d/sub/f").await, "first");
        drop(ctx);

        rename(&f, "d/sub", "old_sub");
        rename(&f, "incoming_sub", "d/sub");
        let mut changes = FileChangeTracker::new();
        renamed(&mut changes, "d/sub", "old_sub");
        renamed(&mut changes, "incoming_sub", "d/sub");
        let ctx = f.transaction(changes).await?;
        assert_eq!(contents(&ctx, "d/sub/f").await, "second");
        drop(ctx);

        rename(&f, "d", "old");
        rename(&f, "incoming", "d");
        let mut changes = FileChangeTracker::new();
        renamed(&mut changes, "d", "old");
        renamed(&mut changes, "incoming", "d");
        let ctx = f.transaction(changes).await?;
        assert_eq!(contents(&ctx, "d/sub/f").await, "third");
        Ok(())
    }

    /// The file watcher loses track of what changed after a directory was replaced, and by then
    /// the directory that was there first is back. The daemon starts over on a fresh root,
    /// which is told of no change, and must not take what the old root read below the
    /// replacing directory for what is there.
    #[tokio::test]
    async fn a_root_that_starts_over_reads_below_a_replaced_directory_again()
    -> buck2_error::Result<()> {
        let mut f = Fixture::new("").await?;
        f.fs.write_file("d/f", "before");
        f.fs.write_file("incoming/f", "after");
        let ctx = f.transaction(FileChangeTracker::new()).await?;
        assert_eq!(contents(&ctx, "d/f").await, "before");
        drop(ctx);

        rename(&f, "d", "old");
        rename(&f, "incoming", "d");
        let mut changes = FileChangeTracker::new();
        renamed(&mut changes, "d", "old");
        renamed(&mut changes, "incoming", "d");
        let ctx = f.transaction(changes).await?;
        assert_eq!(contents(&ctx, "d/f").await, "after");
        drop(ctx);

        rename(&f, "d", "incoming");
        rename(&f, "old", "d");
        f.start_over().await;
        let ctx = f.transaction(FileChangeTracker::new()).await?;
        assert_eq!(contents(&ctx, "d/f").await, "before");
        Ok(())
    }

    /// The contents of a file, as a key of its own: recomputed only when DICE finds the read of
    /// the file changed.
    #[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(dice::DiceKeyDyn)]
    struct Contents(IProjectRelativePath);

    #[async_trait]
    impl Key for Contents {
        type Value = String;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            let path = CellPath::testing_new(&format!("root//{}", self.0));
            DiceFileComputations::read_file_if_exists(ctx, path)
                .await
                .unwrap()
                .unwrap()
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| x == y)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            PagableValueSerialize::<Self::Value>::new()
        }
    }

    /// The computations DICE runs, by key type.
    #[derive(Default, Allocative)]
    struct Computations(#[allocative(skip)] Mutex<BTreeMap<&'static str, usize>>);

    impl DiceEventListener for Computations {
        fn event(&self, ev: DiceEvent) {
            if let DiceEvent::ComputeStarted { key_type } = ev {
                *self.0.lock().unwrap().entry(key_type).or_default() += 1;
            }
        }
    }

    /// `dirs` directories, each with one file more than the one before it so that no two read
    /// alike, and what reading all of it after `changes` has DICE compute.
    struct Tree {
        f: Fixture,
        dirs: usize,
    }

    impl Tree {
        async fn new(dirs: usize) -> buck2_error::Result<Self> {
            let f = Fixture::new("").await?;
            let tree = Self { f, dirs };
            for d in 0..dirs {
                for file in 0..tree.files_in(d) {
                    tree.f.fs.write_file(&format!("tree/d{d}/sub/f{file}"), "");
                }
            }
            tree.read(FileChangeTracker::new()).await?;
            Ok(tree)
        }

        /// Lists every directory, and reads every file and its metadata.
        async fn read(
            &self,
            changes: FileChangeTracker,
        ) -> buck2_error::Result<BTreeMap<&'static str, usize>> {
            let computations = Arc::new(Computations::default());
            let mut data = UserComputationData::new();
            initialize_read_dir_cache(&mut data);
            data.tracker = computations.dupe();
            let mut updater = self.f.dice.updater_with_data(self.f.root, data);
            updater.set_cell_resolver(self.f.cells.dupe())?;
            updater.set_file_system_baseline(self.f.baseline.dupe())?;
            changes.write_to_dice(&mut updater)?;
            let ctx = updater.commit().await;
            read_dir(&ctx, "root//tree").await.unwrap();
            for d in 0..self.dirs {
                read_dir(&ctx, &format!("root//tree/d{d}")).await.unwrap();
                let listed = read_dir(&ctx, &format!("root//tree/d{d}/sub")).await;
                for file in listed.unwrap() {
                    let path = format!("root//tree/d{d}/sub/{file}");
                    read_file(&ctx, &path).await;
                    metadata(&ctx, &path).await;
                }
            }
            drop(ctx);
            let computations = computations.0.lock().unwrap().clone();
            Ok(computations)
        }

        fn files_in(&self, d: usize) -> usize {
            d + 1
        }
    }

    fn computed(counts: &[(&'static str, usize)]) -> BTreeMap<&'static str, usize> {
        counts.iter().copied().collect()
    }

    /// Reading a tree again when nothing was reported computes nothing.
    #[tokio::test]
    async fn nothing_reported_computes_nothing() -> buck2_error::Result<()> {
        let tree = Tree::new(10).await?;
        assert_eq!(tree.read(FileChangeTracker::new()).await?, computed(&[]));
        Ok(())
    }

    /// A file created, modified or deleted has its own reads computed, and the listing of its
    /// directory: nothing of its siblings, and nothing of the directories above it.
    #[tokio::test]
    async fn a_reported_file_computes_its_own_reads() -> buck2_error::Result<()> {
        let tree = Tree::new(10).await?;

        tree.f.fs.write_file("tree/d3/sub/created", "");
        let mut changes = FileChangeTracker::new();
        changes.file_added_or_removed(p("tree/d3/sub/created"));
        // Watchman also reports the directory the file is in as modified, and so does EdenFS
        // for a change of commit.
        changes.dir_entries_changed_force_invalidate(p("tree/d3/sub"));
        changes.dir_entries_changed_for_watchman_bug(p("tree/d3/sub"));
        assert_eq!(
            tree.read(changes).await?,
            computed(&[
                ("IoPathMetadataKey", 1),
                ("IoReadDirKey", 1),
                ("IoReadFileKey", 1),
                ("PathCheckKey", 1),
            ])
        );

        tree.f.fs.write_file("tree/d3/sub/f0", "modified");
        let mut changes = FileChangeTracker::new();
        changes.file_contents_changed(p("tree/d3/sub/f0"));
        assert_eq!(
            tree.read(changes).await?,
            computed(&[
                ("IoPathMetadataKey", 1),
                ("IoReadFileKey", 1),
                ("PathCheckKey", 1),
            ])
        );

        fs_util::remove_file(resolve(&tree.f, "tree/d3/sub/f0")).unwrap();
        let mut changes = FileChangeTracker::new();
        changes.file_added_or_removed(p("tree/d3/sub/f0"));
        changes.dir_entries_changed_force_invalidate(p("tree/d3/sub"));
        changes.dir_entries_changed_for_watchman_bug(p("tree/d3/sub"));
        // The file is not listed any more, so only the listing is asked for again.
        assert_eq!(tree.read(changes).await?, computed(&[("IoReadDirKey", 1)]));
        Ok(())
    }

    /// A directory reported as added or removed has what is below it computed again, and
    /// nothing else.
    #[tokio::test]
    async fn a_reported_directory_computes_what_is_below_it() -> buck2_error::Result<()> {
        let tree = Tree::new(10).await?;
        let files = tree.files_in(3);

        let mut changes = FileChangeTracker::new();
        changes.dir_tree_added_or_removed(p("tree/d3"));
        assert_eq!(
            tree.read(changes).await?,
            computed(&[
                ("IoPathMetadataKey", files),
                // Of `tree`, which the tracker dirties, of `tree/d3` and of `tree/d3/sub`.
                ("IoReadDirKey", 3),
                ("IoReadFileKey", files),
                // Of `tree/d3`, of `tree/d3/sub` and of the files.
                ("PathCheckKey", 2 + files),
            ])
        );
        Ok(())
    }

    /// A directory that source control adds to the commit or removes from it, while it stays on
    /// disk with what is in it.
    #[tokio::test]
    async fn a_directory_reported_with_its_entries_computes_nothing_below_it()
    -> buck2_error::Result<()> {
        let tree = Tree::new(10).await?;

        let mut changes = FileChangeTracker::new();
        changes.dir_added_or_removed(p("tree/d3"));
        assert_eq!(
            tree.read(changes).await?,
            // Of `tree`, which the tracker dirties, and the check.
            computed(&[("IoReadDirKey", 1), ("PathCheckKey", 1)])
        );
        Ok(())
    }

    /// Something reported at the path of a directory, without the directory being reported as
    /// added or removed, leaves what is below it alone. File watchers do that for a directory
    /// whose attributes or entries changed.
    #[tokio::test]
    async fn a_directory_reported_as_a_file_computes_nothing_below_it() -> buck2_error::Result<()> {
        let tree = Tree::new(10).await?;

        let mut changes = FileChangeTracker::new();
        changes.file_added_or_removed(p("tree/d3"));
        changes.file_contents_changed(p("tree/d3/sub"));
        assert_eq!(
            tree.read(changes).await?,
            // Of `tree`, which the tracker dirties, and the two checks.
            computed(&[("IoReadDirKey", 1), ("PathCheckKey", 2)])
        );
        Ok(())
    }
}
