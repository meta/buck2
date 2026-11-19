/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::future::Future;

use dice_core::BranchId;
use dice_core::Version;
use dice_error::DiceResult;
use dupe::Dupe;
use futures::FutureExt;

use crate::api::computations::DiceComputations;
use crate::api::data::DiceData;
use crate::api::key::Key;
use crate::api::user_data::UserComputationData;
use crate::epoch::ctx::TransactionCtx;
use crate::updater::TransactionUpdater;

/// The struct for which we build transactions. This is where changes are recorded, and committed
/// to DICE, which returns the Transaction where we spawn computations.
#[repr(transparent)]
pub struct DiceTransactionUpdater(pub(crate) TransactionUpdater);

// SAFETY: as for `DiceComputations`.
unsafe impl Send for DiceTransactionUpdater {}
unsafe impl Sync for DiceTransactionUpdater {}

fn _assert_updater_send_sync(DiceTransactionUpdater(inner): &DiceTransactionUpdater) {
    fn assert<T: Send + Sync>(_: &T) {}
    assert(inner);
}

impl DiceTransactionUpdater {
    /// The branch this updater commits to.
    pub fn branch(&self) -> BranchId {
        self.0.branch()
    }

    /// Makes `branch` the one this updater commits to. The changes recorded so far go there too,
    /// and `existing_state` follows.
    pub fn retarget(&mut self, branch: BranchId) {
        self.0.retarget(branch)
    }

    /// A transaction at the branch's newest version, without committing the recorded changes.
    pub fn existing_state(&self) -> impl Future<Output = DiceTransaction> {
        self.0.existing_state().map(DiceTransaction)
    }

    /// Records a set of `Key`s as changed so that they, and any dependents will
    /// be recomputed on the next set of requests at the next version.
    pub fn changed<K, I>(&mut self, changed: I) -> DiceResult<()>
    where
        K: Key,
        I: IntoIterator<Item = K> + Send + Sync + 'static,
    {
        self.0.changed(changed)
    }

    /// Records a set of `Key`s as changed to a particular value so that any
    /// dependents will be recomputed on the next set of requests. The
    /// `Key`s themselves will be update to the new value such that they
    /// will not need to be recomputed as long as they aren't recorded to be
    /// `changed` again (or invalidated by other means). Calling this method
    /// does not in anyway alter the types of the key such that they
    /// permanently becomes a special "inject value only" key.
    pub fn changed_to<K, I>(&mut self, changed: I) -> DiceResult<()>
    where
        K: Key,
        I: IntoIterator<Item = (K, K::Value)> + Send + Sync + 'static,
    {
        self.0.changed_to(changed)
    }

    /// Commit the changes registered via 'changed' and 'changed_to' to the current newest version.
    pub fn commit(self) -> impl Future<Output = DiceTransaction> {
        self.0.commit().map(DiceTransaction)
    }

    /// Commit the changes registered via 'changed' and 'changed_to' to the current newest version,
    /// replacing the user data with the given set
    pub fn commit_with_data(
        self,
        extra: UserComputationData,
    ) -> impl Future<Output = DiceTransaction> {
        self.0.commit_with_data(extra).map(DiceTransaction)
    }
}

/// The base struct for which all computations start. This is clonable, and dupe, and can be
/// moved to different runtimes to start computations.
/// All computations on this transaction will see only changes at the most-up-to-date version at
/// the time of creation of this transaction.
///
/// This SHOULD NOT be ever stored by computations, or any results of computations.
#[derive(Clone, Dupe)]
pub struct DiceTransaction(pub(crate) TransactionCtx);

// SAFETY: as for `DiceComputations`.
unsafe impl Send for DiceTransaction {}
unsafe impl Sync for DiceTransaction {}

fn _assert_transaction_send_sync(DiceTransaction(inner): &DiceTransaction) {
    fn assert<T: Send + Sync>(_: &T) {}
    assert(inner);
}

impl DiceTransaction {
    /// The version this transaction runs at. Two transactions at one version see the same state,
    /// the same injected values and the same dirties, and share the computations they request.
    pub fn version(&self) -> Version {
        self.0.get_version()
    }

    /// Request the result of computing a particular key.
    ///
    /// This is similar to `DiceComputations::compute`, but a bit more flexible as it takes `&self`
    /// - the standard requirements around structured dependencies don't apply.
    pub fn compute<'a, K>(
        &'a self,
        key: &K,
    ) -> impl Future<Output = DiceResult<&'a <K as Key>::Value>> + use<'a, K>
    where
        K: Key,
    {
        self.0.compute(key)
    }

    /// Get access to a `DiceComputations` connected to this transaction.
    ///
    /// Usually not needed over `compute`, but can be useful for sharing code.
    pub fn ctx(&self) -> DiceComputations<'_> {
        DiceComputations(self.0.as_computations())
    }

    /// Data that is static for the lifetime of the current request context. This lifetime is
    /// the lifetime of the top-level `DiceComputation` used for all requests.
    /// The data is also specific to each request context, so multiple concurrent requests can
    /// each have their own individual data.
    pub fn per_transaction_data(&self) -> &UserComputationData {
        self.0.per_transaction_data()
    }

    /// Untracked data that is shared by transactions in this branch tree.
    pub fn global_data(&self) -> &DiceData {
        self.0.global_data()
    }
}
