/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::slice;
use std::sync::Arc;

use allocative::Allocative;
use buck2_core::provider::id::ProviderId;
use dupe::Dupe;
use pagable::Pagable;
use strong_hash::StrongHash;

#[derive(
    Debug, Eq, PartialEq, Hash, StrongHash, Clone, Dupe, Allocative, Pagable
)]
pub struct ProviderIdSet(Option<Arc<Vec<Arc<ProviderId>>>>);

impl ProviderIdSet {
    pub const EMPTY: ProviderIdSet = ProviderIdSet(None);

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.providers().is_empty()
    }

    #[inline]
    pub fn providers(&self) -> &[Arc<ProviderId>] {
        match &self.0 {
            None => &[],
            Some(providers) => providers,
        }
    }
}

impl From<Vec<Arc<ProviderId>>> for ProviderIdSet {
    #[inline]
    fn from(mut v: Vec<Arc<ProviderId>>) -> Self {
        if v.is_empty() {
            ProviderIdSet::EMPTY
        } else {
            v.sort_unstable();
            ProviderIdSet(Some(Arc::new(v)))
        }
    }
}

impl<'a> IntoIterator for &'a ProviderIdSet {
    type Item = &'a Arc<ProviderId>;
    type IntoIter = slice::Iter<'a, Arc<ProviderId>>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.providers().iter()
    }
}
