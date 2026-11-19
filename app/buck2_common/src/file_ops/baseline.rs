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
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice::DiceComputations;
use dice::DiceTransactionUpdater;
use dice::EqualityBehavior;
use dice::InjectedKey;
use dice::InvalidationSourcePriority;
use dice::PagableValueSerialize;
use dice::ValueSerialize;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;

/// The state of the project's files that the changes recorded on a DICE branch are relative to.
/// A file reads as it does in this state unless a change to it was recorded since.
///
/// Branches with equal baselines agree on every file neither has recorded a change to, which is
/// what lets one reuse what the other read. A file watcher that cannot name the revision behind
/// the files it finds uses a baseline equal to no other, so that nothing is reused across it.
#[derive(Clone, Dupe, Debug, Eq, PartialEq, Hash, Allocative, Pagable)]
pub enum FileSystemBaseline {
    /// The files as of a source control revision, with every way the working copy differs from
    /// it recorded as a change.
    Revision(Arc<str>),
    /// A state equal to no other. Only [`FileSystemBaseline::unique`] produces these.
    Unique(u64),
}

impl FileSystemBaseline {
    /// A baseline equal to no other.
    pub fn unique() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self::Unique(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("{:?}", self)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct FileSystemBaselineKey;

impl InjectedKey for FileSystemBaselineKey {
    type Value = FileSystemBaseline;

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    /// The baseline is a premise of every file read, not a change to one. Were it a source, the
    /// injection that every daemon startup performs would report each file read in the first build
    /// as invalidated by a file change.
    fn invalidation_source_priority() -> InvalidationSourcePriority {
        InvalidationSourcePriority::Ignored
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        PagableValueSerialize::<Self::Value>::new()
    }
}

#[async_trait]
pub(crate) trait HasFileSystemBaseline<'d> {
    async fn get_file_system_baseline(&mut self) -> buck2_error::Result<&'d FileSystemBaseline>;
}

#[async_trait]
impl<'d> HasFileSystemBaseline<'d> for DiceComputations<'d> {
    async fn get_file_system_baseline(&mut self) -> buck2_error::Result<&'d FileSystemBaseline> {
        Ok(self.compute(&FileSystemBaselineKey).await?)
    }
}

pub trait SetFileSystemBaseline {
    fn set_file_system_baseline(&mut self, baseline: FileSystemBaseline)
    -> buck2_error::Result<()>;
}

impl SetFileSystemBaseline for DiceTransactionUpdater {
    fn set_file_system_baseline(
        &mut self,
        baseline: FileSystemBaseline,
    ) -> buck2_error::Result<()> {
        Ok(self.changed_to(vec![(FileSystemBaselineKey, baseline)])?)
    }
}

#[cfg(test)]
mod tests {
    use buck2_core::cells::CellResolver;
    use buck2_core::cells::cell_path::CellPath;
    use buck2_core::cells::cell_root_path::CellRootPathBuf;
    use buck2_core::cells::name::CellName;
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
    use dice::CancellationContext;
    use dice::DetectCycles;
    use dice::Dice;
    use dice::Key;

    use super::*;
    use crate::dice::cells::SetCellResolver;
    use crate::dice::data::testing::SetTestingIoProvider;
    use crate::file_ops::dice::DiceFileComputations;

    /// The contents of `root//f`.
    #[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(dice::DiceKeyDyn)]
    struct Contents;

    #[async_trait]
    impl Key for Contents {
        type Value = String;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            DiceFileComputations::read_file_if_exists(ctx, CellPath::testing_new("root//f"))
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

    /// What was read under a baseline is reused under the same baseline, and not under another.
    #[tokio::test]
    async fn file_reads_are_reused_only_under_their_baseline() -> buck2_error::Result<()> {
        let fs = ProjectRootTemp::new()?;
        fs.write_file("f", "one");
        let mut dice = Dice::builder();
        dice.set_testing_io_provider(&fs);
        let dice = dice.build(DetectCycles::Enabled);
        let root = dice.new_root().await;
        let first = FileSystemBaseline::unique();

        let mut updater = dice.updater(root);
        updater.set_cell_resolver(CellResolver::testing_with_name_and_path(
            CellName::testing_new("root"),
            CellRootPathBuf::new(ProjectRelativePathBuf::unchecked_new("".to_owned())),
        ))?;
        updater.set_file_system_baseline(first.dupe())?;
        let ctx = updater.commit().await;
        assert_eq!(ctx.compute(&Contents).await?, "one");

        fs.write_file("f", "two");

        let mut updater = dice.updater(root);
        updater.set_file_system_baseline(first)?;
        let ctx = updater.commit().await;
        assert_eq!(ctx.compute(&Contents).await?, "one");

        let mut updater = dice.updater(root);
        updater.set_file_system_baseline(FileSystemBaseline::unique())?;
        let ctx = updater.commit().await;
        assert_eq!(ctx.compute(&Contents).await?, "two");
        Ok(())
    }
}
