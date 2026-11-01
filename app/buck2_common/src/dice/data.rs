/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Core data on dice

use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_core::fs::project::ProjectRoot;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use derive_more::Display;
use dice::DiceComputations;
use dice::DiceData;
use dice::DiceDataBuilder;
use dice::DiceTransactionUpdater;
use dice::EqualityBehavior;
use dice::InjectedKey;
use dice::PagableValueSerialize;
use dice::ValueSerialize;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;

use crate::cas_digest::CasDigestConfig;
use crate::io::IoProvider;

pub trait HasIoProvider {
    fn get_io_provider(&self) -> &dyn IoProvider;
}

pub trait SetIoProvider {
    fn set_io_provider(&mut self, fs: Arc<dyn IoProvider>);
}

impl HasIoProvider for DiceData {
    fn get_io_provider(&self) -> &dyn IoProvider {
        &**self
            .get::<Arc<dyn IoProvider>>()
            .expect("project filesystem should be set")
    }
}

impl SetIoProvider for DiceDataBuilder {
    fn set_io_provider(&mut self, fs: Arc<dyn IoProvider>) {
        self.set(fs)
    }
}

impl SetIoProvider for DiceData {
    fn set_io_provider(&mut self, fs: Arc<dyn IoProvider>) {
        self.set(fs)
    }
}

#[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct IsolationDirKey;

impl InjectedKey for IsolationDirKey {
    type Value = AbsNormPathBuf;

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|a, b| a == b)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        PagableValueSerialize::<Self::Value>::new()
    }
}

#[async_trait]
pub trait HasIsolationDir<'d> {
    /// The absolute output directory of the current tenant.
    async fn get_isolation_dir(&mut self) -> buck2_error::Result<&'d AbsNormPathBuf>;
}

#[async_trait]
impl<'d> HasIsolationDir<'d> for DiceComputations<'d> {
    async fn get_isolation_dir(&mut self) -> buck2_error::Result<&'d AbsNormPathBuf> {
        Ok(self.compute(&IsolationDirKey).await?)
    }
}

#[async_trait]
pub trait HasProjectRoot<'d> {
    /// The project root associated with the tracked tenant isolation directory.
    async fn get_project_root(&mut self) -> buck2_error::Result<&'d ProjectRoot>;
}

#[async_trait]
impl<'d> HasProjectRoot<'d> for DiceComputations<'d> {
    async fn get_project_root(&mut self) -> buck2_error::Result<&'d ProjectRoot> {
        self.get_isolation_dir().await?;
        Ok(self.global_data().get_io_provider().project_root())
    }
}

pub trait SetIsolationDir {
    /// Set the absolute output directory of the current tenant.
    fn set_isolation_dir(&mut self, path: AbsNormPathBuf) -> buck2_error::Result<()>;
}

impl SetIsolationDir for DiceTransactionUpdater {
    fn set_isolation_dir(&mut self, path: AbsNormPathBuf) -> buck2_error::Result<()> {
        Ok(self.changed_to(vec![(IsolationDirKey, path)])?)
    }
}

pub mod testing {
    use buck2_core::fs::project::ProjectRootTemp;

    use super::*;
    use crate::ignores::ignore_set::IgnoreSet;
    use crate::ignores::ignore_set::SetIgnoreSet;
    use crate::io::fs::FsIoProvider;

    pub trait SetTestingIoProvider {
        /// Reads come from `fs`, with nothing ignored. A test that wants ignores sets its own
        /// `IgnoreSet` afterwards.
        fn set_testing_io_provider(&mut self, fs: &ProjectRootTemp);
    }

    impl SetTestingIoProvider for DiceDataBuilder {
        fn set_testing_io_provider(&mut self, fs: &ProjectRootTemp) {
            self.set_io_provider(Arc::new(FsIoProvider::new(
                fs.path().dupe(),
                CasDigestConfig::testing_default(),
                false,
            )));
            self.set_ignore_set(Arc::new(IgnoreSet::testing_empty()));
        }
    }
}
