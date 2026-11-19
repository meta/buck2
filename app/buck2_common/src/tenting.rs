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

use async_trait::async_trait;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use dice::DiceData;
use dice::DiceDataBuilder;
use dupe::Dupe;

/// Outcome of a tenting (path-based ACL) lookup. `Unknown` is kept distinct from
/// `NotTented` so a transient lookup failure is never mistaken for "not tented"
/// (only `NotTented` may clear stored tenting metadata downstream).
#[derive(Debug, Clone, Dupe, PartialEq, Eq)]
pub enum TentingStatus {
    /// The path is protected by one or more ACLs. Always carries at least one
    /// ACL name.
    Tented(Arc<[String]>),
    /// The lookup succeeded and the path is under no ACL.
    NotTented,
    /// Tenting could not be determined (EdenFS API failure, ACLs attribute not
    /// yet deployed, empty response, or under an ACL with no resolvable names).
    /// Best-effort: callers must NOT treat this as "not tented".
    Unknown,
}

#[async_trait]
pub trait TentingAclProvider: Send + Sync + 'static {
    /// Returns the tenting status of a path. See [`TentingStatus`] for how the
    /// "not tented" and "could not determine" cases are distinguished.
    async fn get_tenting_acl_names(
        &self,
        path: &ProjectRelativePath,
    ) -> buck2_error::Result<TentingStatus>;
}

pub trait HasTentingAclProvider {
    fn get_tenting_acl_provider(&self) -> Option<Arc<dyn TentingAclProvider>>;
}

pub trait SetTentingAclProvider {
    fn set_tenting_acl_provider(&mut self, provider: Option<Arc<dyn TentingAclProvider>>);
}

impl HasTentingAclProvider for DiceData {
    fn get_tenting_acl_provider(&self) -> Option<Arc<dyn TentingAclProvider>> {
        self.get::<Option<Arc<dyn TentingAclProvider>>>()
            .expect("TentingAclProvider should be set")
            .as_ref()
            .map(|p| p.dupe())
    }
}

impl SetTentingAclProvider for DiceDataBuilder {
    fn set_tenting_acl_provider(&mut self, provider: Option<Arc<dyn TentingAclProvider>>) {
        self.set(provider)
    }
}

impl SetTentingAclProvider for DiceData {
    fn set_tenting_acl_provider(&mut self, provider: Option<Arc<dyn TentingAclProvider>>) {
        self.set(provider)
    }
}
