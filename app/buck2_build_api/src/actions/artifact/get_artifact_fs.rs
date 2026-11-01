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
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::dice::data::HasProjectRoot;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
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

use crate::context::HasBuildContextData;

#[async_trait]
pub trait GetArtifactFs<'d> {
    /// Get the configured ArtifactFs.
    async fn get_artifact_fs(&mut self) -> buck2_error::Result<&'d ArtifactFs>;
}

#[async_trait]
impl<'d> GetArtifactFs<'d> for DiceComputations<'d> {
    async fn get_artifact_fs(&mut self) -> buck2_error::Result<&'d ArtifactFs> {
        self.compute(&ArtifactFsKey).await?.as_ref().duped_err()
    }
}

#[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("{:?}", self)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct ArtifactFsKey;

#[async_trait]
impl Key for ArtifactFsKey {
    type Value = buck2_error::Result<ArtifactFs>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        let buck_out_path_resolver = ctx.get_buck_out_path().await?.dupe();
        let cell_resolver = ctx.get_cell_resolver().await?.dupe();
        let project_filesystem = ctx.get_project_root().await?.dupe();
        Ok(ArtifactFs::new(
            cell_resolver,
            buck_out_path_resolver,
            project_filesystem,
        ))
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| match (x, y) {
            (Ok(x), Ok(y)) => x == y,
            (_, _) => false,
        })
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        OkPagableValueSerialize::<Self::Value>::new()
    }
}
