/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;
use std::fmt::Display;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;

use buck2_core::package::PackageLabel;
use buck2_hash::BuckHasher;
use cmp_any::PartialEqAny;
use dupe::Dupe;

/// `Display` should not include type name (like `dice::Key`).
pub trait BuildSignalsNodeKeyImpl:
    Eq + PartialEq + Hash + Display + Debug + Send + Sync + 'static
{
    fn critical_path_entry_proto(&self) -> Option<buck2_data::critical_path_entry2::Entry> {
        None
    }

    fn kind(&self) -> &'static str {
        "unknown"
    }

    /// The package of the target this computation is about, for computations
    /// (configuring a target, resolving its select keys) whose dependencies on
    /// other packages count as discovering those packages.
    fn package(&self) -> Option<PackageLabel> {
        None
    }
}

pub trait BuildSignalsNodeKeyDyn: Send + Sync + 'static {
    fn eq_token(&self) -> PartialEqAny<'_>;
    fn dislpay(&self) -> &dyn Display;
    fn debug(&self) -> &dyn Debug;
    fn critical_path_entry_proto(&self) -> Option<buck2_data::critical_path_entry2::Entry>;
    fn hash(&self) -> u64;
    fn kind(&self) -> &'static str;
    fn package(&self) -> Option<PackageLabel>;
}

impl<T: BuildSignalsNodeKeyImpl> BuildSignalsNodeKeyDyn for T {
    fn eq_token(&self) -> PartialEqAny<'_> {
        PartialEqAny::new(self)
    }

    fn dislpay(&self) -> &dyn Display {
        self
    }

    fn debug(&self) -> &dyn Debug {
        self
    }

    fn critical_path_entry_proto(&self) -> Option<buck2_data::critical_path_entry2::Entry> {
        self.critical_path_entry_proto()
    }

    fn hash(&self) -> u64 {
        let mut hasher = BuckHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }

    fn kind(&self) -> &'static str {
        BuildSignalsNodeKeyImpl::kind(self)
    }

    fn package(&self) -> Option<PackageLabel> {
        BuildSignalsNodeKeyImpl::package(self)
    }
}

#[derive(Clone, Dupe)]
pub struct BuildSignalsNodeKey(Arc<dyn BuildSignalsNodeKeyDyn>);

impl BuildSignalsNodeKey {
    pub fn new<T: BuildSignalsNodeKeyImpl + 'static>(key: T) -> Self {
        BuildSignalsNodeKey(Arc::new(key))
    }

    pub fn critical_path_entry_proto(&self) -> Option<buck2_data::critical_path_entry2::Entry> {
        self.0.critical_path_entry_proto()
    }

    pub fn package(&self) -> Option<PackageLabel> {
        self.0.package()
    }

    pub fn kind(&self) -> &'static str {
        self.0.kind()
    }
}

impl PartialEq for BuildSignalsNodeKey {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_token() == other.0.eq_token()
    }
}

impl Eq for BuildSignalsNodeKey {}

impl Display for BuildSignalsNodeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self.0.dislpay(), f)
    }
}

impl Debug for BuildSignalsNodeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(self.0.debug(), f)
    }
}

impl Hash for BuildSignalsNodeKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.0.hash());
    }
}
