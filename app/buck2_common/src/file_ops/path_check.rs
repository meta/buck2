/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use async_trait::async_trait;
use buck2_core::fs::i_project_rel_path::IProjectRelativePath;
use buck2_core::soft_error;
use buck2_error::BuckErrorContext;
use derive_more::Display;
use dice::CancellationContext;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dupe::Dupe;
use dupe::ResultDupedErrExt;
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
    ctx.compute(&PathCheckKey(path)).await?.dupe()
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
/// there depends on the file watcher, which reports no changes to ignored paths.
pub(super) async fn check_ignored_path(
    ctx: &mut DiceComputations<'_>,
    path: IProjectRelativePath,
    at: IProjectRelativePath,
) -> buck2_error::Result<PathCheck> {
    let mut below = path
        .strip_prefix(at)
        .internal_error("the check reported a prefix")?
        .iter();
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
/// through the dependency of each check on its parent's.
///
/// The listing of the parent is read through the per-transaction directory cache rather than
/// `IoReadDirKey`, so that checking a path records no dependency on its parent's listing. Were it
/// to, adding a file to a directory would invalidate every read under each of its siblings;
/// `tests/core/fs/invalidations/test_root_directory.py` guards against this.
#[derive(
    Clone, Dupe, Copy, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable
)]
#[pagable_typetag(dice::DiceKeyDyn)]
pub(super) struct PathCheckKey(pub(super) IProjectRelativePath);

#[async_trait]
impl Key for PathCheckKey {
    type Value = buck2_error::Result<PathCheck>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        ctx.get_file_system_baseline().await?;
        let Some((parent, name)) = self.0.split_last() else {
            return Ok(PathCheck::Present(FileType::Directory));
        };
        match *ctx
            .compute(&PathCheckKey(parent))
            .await?
            .as_ref()
            .duped_err()?
        {
            PathCheck::Present(FileType::Directory) => {}
            PathCheck::Present(FileType::Symlink) => {
                return Ok(PathCheck::ParentSymlink { at: parent });
            }
            PathCheck::Present(file_type) => {
                return Ok(PathCheck::ParentNotDirectory {
                    at: parent,
                    file_type,
                });
            }
            inherited @ (PathCheck::Missing { .. }
            | PathCheck::Ignored { .. }
            | PathCheck::ParentSymlink { .. }
            | PathCheck::ParentNotDirectory { .. }) => return Ok(inherited),
        }
        if ctx.global_data().get_ignore_set().is_match(&self.0) {
            return Ok(PathCheck::Ignored { at: self.0 });
        }
        Ok(match read_dir_cached(ctx, parent).await?.lookup(name) {
            Some(file_type) => PathCheck::Present(file_type),
            None => PathCheck::Missing { at: self.0 },
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
    use std::sync::Arc;

    use assert_matches::assert_matches;
    use buck2_core::cells::CellResolver;
    use buck2_core::cells::cell_path::CellPath;
    use buck2_core::cells::cell_root_path::CellRootPathBuf;
    use buck2_core::cells::name::CellName;
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;
    use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
    use buck2_fs::fs_util;
    use dice::DetectCycles;
    use dice::Dice;
    use dice::DiceTransaction;
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
        // Read through, with a soft error, until `read_through_symlink` is retired.
        assert_eq!(read_dir(&ctx, "root//l").await, names(&["b"]));
        assert_eq!(read_dir(&ctx, "root//l/b").await, names(&["f"]));
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
        // Read through, with a soft error, until `read_through_symlink` is retired.
        assert!(exists("root//l/b").await);
        assert!(!exists("root//l/nope").await);
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

    /// File reads go through the check: absent and ignored files read as `None`, directories are
    /// errors.
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
        // Read through, with a soft error, until `read_through_symlink` is retired.
        assert_eq!(read("root//l/f").await, Ok(Some("contents".to_owned())));
        assert_eq!(read("root//a/lf").await, Ok(Some("contents".to_owned())));
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
}
