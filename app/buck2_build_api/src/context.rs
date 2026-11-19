/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Injects data used for build onto dice

use allocative::Allocative;
use async_trait::async_trait;
use buck2_core::fs::buck_out_path::BuckOutPathResolver;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use derive_more::Display;
use dice::DiceComputations;
use dice::DiceTransactionUpdater;
use dice::EqualityBehavior;
use dice::InjectedKey;
use dice::PagableValueSerialize;
use dice::ValueSerialize;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;

#[async_trait]
pub trait HasBuildContextData<'d> {
    async fn get_buck_out_path(&mut self) -> buck2_error::Result<&'d BuckOutPathResolver>;
}

pub trait SetBuildContextData {
    fn set_buck_out_path(
        &mut self,
        path: Option<ProjectRelativePathBuf>,
    ) -> buck2_error::Result<()>;
}

#[derive(PartialEq, Eq, Allocative, Pagable)]
pub struct BuildData {
    buck_out_path_resolver: BuckOutPathResolver,
}

#[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("{:?}", self)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct BuildDataKey;

impl InjectedKey for BuildDataKey {
    type Value = BuildData;

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        PagableValueSerialize::<Self::Value>::new()
    }
}

#[async_trait]
impl<'d> HasBuildContextData<'d> for DiceComputations<'d> {
    async fn get_buck_out_path(&mut self) -> buck2_error::Result<&'d BuckOutPathResolver> {
        let data = self.compute(&BuildDataKey).await?;
        Ok(&data.buck_out_path_resolver)
    }
}

impl SetBuildContextData for DiceTransactionUpdater {
    fn set_buck_out_path(
        &mut self,
        path: Option<ProjectRelativePathBuf>,
    ) -> buck2_error::Result<()> {
        Ok(self.changed_to(vec![(
            BuildDataKey,
            BuildData {
                buck_out_path_resolver: BuckOutPathResolver::new(path.unwrap_or_else(|| {
                    ProjectRelativePathBuf::unchecked_new("buck-out/v2".to_owned())
                })),
            },
        )])?)
    }
}
