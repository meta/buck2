/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::marker::PhantomData;

use derivative::Derivative;

use crate::api::invalidation_tracking::InvalidationSources;
use crate::api::key::Key;
use crate::core::graph::revision::Revision;
use crate::key::DiceKey;
use crate::value::MaybeValidDiceValue;

#[derive(Derivative)]
#[derivative(Debug)]
pub struct OpaqueValue<'d, K: Key> {
    pub(crate) derive_from_key: DiceKey,
    #[derivative(Debug = "ignore")]
    pub(crate) derive_from: &'d MaybeValidDiceValue,
    /// The [`Revision`] of `derive_from`'s value, `None` iff the value is transient.
    /// Recorded on the dep edge when this `OpaqueValue` is folded into the caller's deps.
    pub(crate) revision: Option<Revision>,
    pub(crate) invalidation_sources: InvalidationSources,
    ty: PhantomData<K>,
}

impl<'d, K> OpaqueValue<'d, K>
where
    K: Key,
{
    pub(crate) fn new(
        derive_from_key: DiceKey,
        derive_from: &'d MaybeValidDiceValue,
        revision: Option<Revision>,
        invalidation_sources: InvalidationSources,
    ) -> Self {
        Self {
            derive_from_key,
            derive_from,
            revision,
            invalidation_sources,
            ty: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc as StdArc;

    use allocative::Allocative;
    use async_trait::async_trait;
    use derive_more::Display;
    use dice_futures::cancellation::CancellationContext;
    use pagable::Pagable;
    use pagable::pagable_typetag;

    use crate::Dice;
    use crate::DiceComputations;
    use crate::DiceKeyDyn;
    use crate::HashSet;
    use crate::api::data::DiceData;
    use crate::api::invalidation_tracking::InvalidationSources;
    use crate::api::key::Key;
    use crate::api::key::NoValueSerialize;
    use crate::api::key::ValueSerialize;
    use crate::key::DiceKey;
    use crate::opaque::OpaqueValue;
    use crate::value::DiceKeyValue;
    use crate::value::DiceValidity;
    use crate::value::MaybeValidDiceValue;

    #[derive(Allocative, Clone, Hash, Eq, PartialEq, Debug, Display, Pagable)]
    #[pagable_typetag(DiceKeyDyn)]
    struct K;

    #[async_trait]
    impl Key for K {
        type Value = i32;

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
    async fn opaque_records_deps_when_used() {
        let dice = Dice::testing_new(DiceData::new());
        let root = dice.new_root().await;

        let ctx = dice.updater(root).commit().await.0;
        let mut ctx = ctx.as_computations();

        let v =
            MaybeValidDiceValue::new(StdArc::new(DiceKeyValue::<K>::new(1)), DiceValidity::Valid);
        let opaque =
            OpaqueValue::<K>::new(DiceKey { index: 0 }, &v, None, InvalidationSources::NONE);

        assert_eq!(ctx.recorded_deps_for_test(), HashSet::default());

        assert_eq!(*ctx.opaque_into_value(opaque), 1);

        assert_eq!(
            ctx.recorded_deps_for_test(),
            [DiceKey { index: 0 }].into_iter().collect()
        );
    }
}
