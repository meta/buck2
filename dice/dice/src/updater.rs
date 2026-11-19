/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc as StdArc;

use allocative::Allocative;
use dice_core::BranchId;
use dice_error::DiceError;
use dice_error::DiceResult;
use dupe::Dupe;

use crate::Dice;
use crate::HashMap;
use crate::api::data::DiceData;
use crate::api::key::InvalidationSourcePriority;
use crate::api::key::Key;
use crate::api::storage_type::StorageType;
use crate::api::user_data::UserComputationData;
use crate::arc::Arc;
use crate::core::state::CoreStateHandle;
use crate::epoch::ctx::TransactionCtx;
use crate::epoch::evaluator::TransactionData;
use crate::epoch::evaluator::VersionState;
use crate::key::DiceKey;
use crate::value::DiceKeyValue;
use crate::value::DiceValidValue;
use crate::value::DiceValidity;
use crate::value::MaybeValidDiceValue;
use crate::versions::VersionNumber;

/// Records changes to commit at the head of one branch.
pub(crate) struct TransactionUpdater {
    dice: StdArc<Dice>,
    branch: BranchId,
    global_data: StdArc<DiceData>,
    scheduled_changes: Changes,
    user_data: Arc<UserComputationData>,
}

impl TransactionUpdater {
    pub(crate) fn new(
        dice: StdArc<Dice>,
        branch: BranchId,
        user_data: Arc<UserComputationData>,
    ) -> Self {
        let global_data = dice.data_for_branch(branch);
        Self {
            dice: dice.dupe(),
            branch,
            global_data,
            scheduled_changes: Changes::new(dice),
            user_data,
        }
    }

    pub(crate) fn branch(&self) -> BranchId {
        self.branch
    }

    pub(crate) fn retarget(&mut self, branch: BranchId) {
        self.branch = branch;
        self.global_data = self.dice.data_for_branch(branch);
    }

    /// Records a set of `Key`s as changed so that they, and any dependents will
    /// be recomputed on the next set of requests at the next version.
    pub(crate) fn changed<K, I>(&mut self, changed: I) -> DiceResult<()>
    where
        K: Key,
        I: IntoIterator<Item = K> + Send + Sync + 'static,
    {
        changed
            .into_iter()
            .try_for_each(|k| self.scheduled_changes.change(k, ChangeType::Invalidate))
    }

    /// Records a set of `Key`s as changed to a particular value so that any
    /// dependents will be recomputed on the next set of requests. The
    /// `Key`s themselves will be update to the new value such that they
    /// will not need to be recomputed as long as they aren't recorded to be
    /// `changed` again (or invalidated by other means). Calling this method
    /// does not in anyway alter the types of the key such that they
    /// permanently becomes a special "inject value only" key.
    pub(crate) fn changed_to<K, I>(&mut self, changed: I) -> DiceResult<()>
    where
        K: Key,
        I: IntoIterator<Item = (K, K::Value)> + Send + Sync + 'static,
    {
        changed.into_iter().try_for_each(|(k, new_value)| {
            match MaybeValidDiceValue::new(
                StdArc::new(DiceKeyValue::<K>::new(new_value)),
                DiceValidity::Valid,
            )
            .into_valid_value()
            {
                Ok(validated_value) => self.scheduled_changes.change(
                    k,
                    ChangeType::UpdateValue(validated_value, K::storage_type()),
                ),
                Err(_) => Err(DiceError::invalid_change(StdArc::new(k))),
            }
        })
    }

    /// Commit the changes registered via 'changed' and 'changed_to' to the current newest version.
    pub(crate) async fn commit(self) -> TransactionCtx {
        let user_data = self.user_data.dupe();
        let dice = self.dice.dupe();
        let global_data = self.global_data.dupe();

        let (transaction, guard) = self.commit_to_state().await;

        TransactionCtx::new(
            TransactionData::new(transaction, user_data, dice, global_data),
            guard,
        )
    }

    /// Commit the changes registered via 'changed' and 'changed_to' to the current newest version,
    /// replacing the user data with the given set
    pub(crate) async fn commit_with_data(self, extra: UserComputationData) -> TransactionCtx {
        let dice = self.dice.dupe();
        let global_data = self.global_data.dupe();

        let (transaction, guard) = self.commit_to_state().await;

        TransactionCtx::new(
            TransactionData::new(transaction, Arc::new(extra), dice, global_data),
            guard,
        )
    }

    pub(crate) async fn existing_state(&self) -> TransactionCtx {
        let v = self.dice.state_handle.current_version(self.branch).await;
        let guard = ActiveTransactionGuard::new(v, self.dice.state_handle.dupe());
        let (transaction, guard) = self.dice.state_handle.ctx_at_version(v, guard).await;
        TransactionCtx::new(
            TransactionData::new(
                transaction,
                self.user_data.dupe(),
                self.dice.dupe(),
                self.global_data.dupe(),
            ),
            guard,
        )
    }

    async fn commit_to_state(self) -> (VersionState, ActiveTransactionGuard) {
        let v = self
            .dice
            .state_handle
            .update_state(
                self.branch,
                self.scheduled_changes
                    .changes
                    .into_iter()
                    .map(|(k, (t, p))| (k, t, p))
                    .collect(),
            )
            .await;

        let guard = ActiveTransactionGuard::new(v, self.dice.state_handle.dupe());

        self.dice.state_handle.ctx_at_version(v, guard).await
    }
}

#[derive(Debug, Dupe, Clone)]
#[expect(dead_code, reason = "Field is only there to be kept alive")]
pub(crate) struct ActiveTransactionGuard(Arc<ActiveTransactionGuardInner>);

impl ActiveTransactionGuard {
    pub(crate) fn new(v: VersionNumber, state_handle: CoreStateHandle) -> Self {
        Self(Arc::new(ActiveTransactionGuardInner { v, state_handle }))
    }
}

#[derive(Debug)]
pub(crate) struct ActiveTransactionGuardInner {
    v: VersionNumber,
    state_handle: CoreStateHandle,
}

impl Drop for ActiveTransactionGuardInner {
    fn drop(&mut self) {
        self.state_handle.drop_ctx_at_version(self.v)
    }
}

struct Changes {
    changes: HashMap<DiceKey, (ChangeType, InvalidationSourcePriority)>,
    dice: StdArc<Dice>,
}

impl Changes {
    pub(crate) fn new(dice: StdArc<Dice>) -> Self {
        Self {
            changes: HashMap::default(),
            dice,
        }
    }

    pub(crate) fn change<K: Key>(&mut self, key: K, change: ChangeType) -> DiceResult<()> {
        match (change, K::storage_type()) {
            (ChangeType::Invalidate, StorageType::Injected) => {
                Err(DiceError::injected_key_invalidated(StdArc::new(key)))
            }
            (change, _) => {
                let key = self.dice.key_index.index_key(key);
                if self
                    .changes
                    .insert(key, (change, K::invalidation_source_priority()))
                    .is_some()
                {
                    Err(DiceError::duplicate(
                        self.dice.key_index.get(key).dupe().downcast::<K>().unwrap(),
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }
}

#[derive(Allocative, Debug)]
pub(crate) enum ChangeType {
    /// Just invalidate the key
    Invalidate,
    /// Update the key to the given value. A `StorageType::Injected` key takes no part in paging;
    /// a computed key asserted this way pages like one that was computed.
    UpdateValue(DiceValidValue, StorageType),
}

#[cfg(test)]
mod tests {

    use allocative::Allocative;
    use assert_matches::assert_matches;
    use async_trait::async_trait;
    use derive_more::Display;
    use dice_futures::cancellation::CancellationContext;
    use pagable::Pagable;
    use pagable::pagable_typetag;

    use crate::DiceKeyDyn;
    use crate::api::computations::DiceComputations;
    use crate::api::data::DiceData;
    use crate::api::key::InvalidationSourcePriority;
    use crate::api::key::Key;
    use crate::api::key::NoValueSerialize;
    use crate::api::key::ValueSerialize;
    use crate::dice::Dice;
    use crate::key::CowDiceKeyHashed;
    use crate::updater::ChangeType;
    use crate::versions::VersionNumber;

    #[derive(Allocative, Clone, PartialEq, Eq, Hash, Debug, Display, Pagable)]
    #[pagable_typetag(DiceKeyDyn)]
    struct K(usize);

    #[async_trait]
    impl Key for K {
        type Value = usize;

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            unimplemented!("test")
        }

        fn equality_behavior() -> crate::EqualityBehavior<Self::Value> {
            crate::EqualityBehavior::Compare(|_x, _y| unimplemented!("test"))
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    #[tokio::test]
    async fn changes_are_recorded() -> anyhow::Result<()> {
        let dice = Dice::testing_new(DiceData::new());
        let root = dice.new_root().await;
        let mut updater = dice.updater(root);

        updater.changed(vec![K(1), K(2)])?;

        updater.changed_to(vec![(K(3), 3), (K(4), 4)])?;

        assert_matches!(
            updater
                .0
                .scheduled_changes
                .changes
                .get(&dice.key_index.index(CowDiceKeyHashed::key(K(1)))),
            Some((ChangeType::Invalidate, InvalidationSourcePriority::Normal))
        );
        assert_matches!(
            updater
                .0
                .scheduled_changes
                .changes
                .get(&dice.key_index.index(CowDiceKeyHashed::key(K(2)))),
            Some((ChangeType::Invalidate, InvalidationSourcePriority::Normal))
        );

        assert_matches!(
        updater
            .0.scheduled_changes
            .changes
            .get(&dice.key_index.index(CowDiceKeyHashed::key(K(3)))),
        Some((ChangeType::UpdateValue(x, _), _)) if *x.downcast_ref::<usize>().unwrap() == 3
            );

        assert_matches!(
        updater
            .0.scheduled_changes
            .changes
            .get(&dice.key_index.index(CowDiceKeyHashed::key(K(4)))),
        Some((ChangeType::UpdateValue(x, _), _)) if *x.downcast_ref::<usize>().unwrap() == 4
            );

        assert!(updater.changed(vec![K(1)]).is_err());

        Ok(())
    }

    #[tokio::test]
    async fn transaction_versions() -> anyhow::Result<()> {
        let dice = Dice::testing_new(DiceData::new());
        let root = dice.new_root().await;
        let mut updater = dice.updater(root);

        updater.changed(vec![K(1), K(2)])?;

        let ctx = updater.existing_state().await;
        assert_eq!(ctx.0.get_version(), VersionNumber::testing_new(1));

        let ctx = updater.commit().await;
        assert_eq!(ctx.0.get_version(), VersionNumber::testing_new(2));

        Ok(())
    }
}
