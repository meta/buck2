/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//!
//! The dynamic incremental caching computation engine that powers buckv2.
//!
//! The computation engine will output values corresponding to given `Key`s,
//! reusing previously computed values when possible. `Key`s computations are
//! allowed to request other `Key`s via a `ComputationContext`.
//!
//! Example:
//! ```
//! mod c {
//!
//!     /// declaring computations:
//!     use async_trait::async_trait;
//!     use derive_more::Display;
//!     use dice::{Key, InjectedKey, DiceComputations, DiceDataBuilder, DiceData, DiceTransactionUpdater};
//!     use std::sync::Arc;
//!     use allocative::Allocative;
//!     use dice_futures::cancellation::CancellationContext;
//!     use dice::DiceKeyDyn;
//!     use dice::EqualityBehavior;
//!     use dice::ValueSerialize;
//!     use dice::NoValueSerialize;
//!     use dupe::Dupe;
//!     use pagable::Pagable;
//!     use pagable::pagable_typetag;
//!
//!     /// A configuration computation that consists of values that are pre-computed outside of DICE
//!     pub struct InjectConfigs<'compute, 'd>(&'compute mut DiceComputations<'d>);
//!
//!     impl<'compute, 'd> InjectConfigs<'compute, 'd> {
//!         pub async fn get_config(&mut self) -> usize {
//!             *self.0.compute(&ConfigKey).await.unwrap()
//!         }
//!     }
//!
//!     #[derive(Clone, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
//!     #[display("{:?}", self)]
//!     #[pagable_typetag(DiceKeyDyn)]
//!     struct ConfigKey;
//!
//!     #[async_trait]
//!     impl InjectedKey for ConfigKey {
//!         type Value = usize;
//!
//!         fn equality_behavior() -> EqualityBehavior<Self::Value> {
//!             EqualityBehavior::Compare(|x, y| x == y)
//!         }
//!         
//!         fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
//!             NoValueSerialize::<Self::Value>::new()
//!         }
//!     }
//!
//!     pub struct MyComputation<'compute, 'd>(pub &'compute mut DiceComputations<'d>);
//!
//!     impl<'compute, 'd> MyComputation<'compute, 'd> {
//!         // declaring a computation function
//!         pub async fn compute_a(&mut self, a: usize, s: String) -> Arc<String> {
//!             #[derive(Clone, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
//!             #[display("{:?}", self)]
//!             #[pagable_typetag(DiceKeyDyn)]
//!             struct ComputeA(usize, String);
//!
//!             #[async_trait]
//!             impl Key for ComputeA {
//!                 type Value = Arc<String>;
//!
//!                 async fn compute(&self, ctx: &mut DiceComputations, _cancellations: &CancellationContext) -> Self::Value {
//!                     // request for other computations on the self
//!                     let n = MyComputation(ctx).compute_b(self.0).await;
//!                     Arc::new(self.1.repeat(n))
//!                 }
//!
//!                 fn equality_behavior() -> EqualityBehavior<Self::Value> {
//!                     EqualityBehavior::Compare(|x, y| x == y)
//!                 }
//!
//!                 fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
//!                     NoValueSerialize::<Self::Value>::new()
//!                 }
//!             }
//!
//!             self.0.compute(&ComputeA(a, s)).await.unwrap().dupe()
//!         }
//!
//!         // second computation function
//!         pub async fn compute_b(&mut self, a: usize) -> usize {
//!                 *self.0.compute(&ComputeB(a)).await.unwrap()
//!         }
//!     }
//!
//!     #[derive(Clone, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
//!     #[display("{:?}", self)]
//!     #[pagable_typetag(DiceKeyDyn)]
//!     struct ComputeB(usize);
//!
//!     #[async_trait]
//!     impl Key for ComputeB {
//!         type Value = usize;
//!
//!         async fn compute(&self, ctx: &mut DiceComputations, cancellations: &CancellationContext) -> Self::Value {
//!             self.0 + InjectConfigs(ctx).get_config().await + ctx.global_data().static_data().len()
//!         }
//!
//!         fn equality_behavior() -> EqualityBehavior<Self::Value> {
//!             EqualityBehavior::Compare(|x, y| x == y)
//!         }
//!
//!         fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
//!             NoValueSerialize::<Self::Value>::new()
//!         }
//!     }
//!
//!     pub trait SetInjectedConfig {
//!         fn inject_config(&mut self, i: usize);
//!     }
//!
//!     impl SetInjectedConfig for DiceTransactionUpdater {
//!         /// ways to inject the precomputed values to dice
//!         fn inject_config(&mut self, i: usize) {
//!             self.changed_to(vec![(ConfigKey, i)]).unwrap()
//!         }
//!     }
//!
//!     pub trait StaticData {
//!         fn static_data(&self) -> &String;
//!     }
//!
//!     impl StaticData for DiceData {
//!         fn static_data(&self) -> &String {
//!             self.get::<String>().unwrap()
//!         }
//!     }
//!
//!     pub trait SetStaticData {
//!         fn set_static(&mut self, s: String);
//!     }
//!
//!     impl SetStaticData for DiceDataBuilder {
//!         fn set_static(&mut self, s: String) {
//!             self.set(s);
//!         }
//!     }
//! }
//!
//! /// how to use computations
//! use dice::{Dice, DetectCycles};
//! use std::sync::Arc;
//! use c::*;
//!
//! let mut rt = tokio::runtime::Runtime::new().unwrap();
//! let mut builder = Dice::builder();
//! builder.set_static("len4".into());
//! let engine = builder.build(DetectCycles::Disabled);
//! let root = rt.block_on(engine.new_root());
//!
//! // inject config
//! let mut ctx = engine.updater(root);
//! ctx.inject_config(0);
//!
//! let mut ctx = rt.block_on(ctx.commit());
//!
//! // request the computation from DICE
//! rt.block_on(async {
//!     assert_eq!("aaaaaaaa", &*MyComputation(&mut ctx.ctx()).compute_a(4, "a".into()).await);
//! });
//!
//! let mut ctx = engine.updater(root);
//! ctx.inject_config(2);
//!
//! let mut ctx = rt.block_on(ctx.commit());
//!
//! // request the computation from DICE
//! rt.block_on(async {
//!     assert_eq!("aaaaaaaaaa", &*MyComputation(&mut ctx.ctx()).compute_a(4, "a".into()).await);
//! });
//! ```

pub mod testing {
    use crate::Dice;
    use crate::DiceDataBuilder;
    use crate::api::cycles::DetectCycles;
    use crate::api::key::Key;
    use crate::api::transaction::DiceTransactionUpdater;
    use crate::api::user_data::UserComputationData;

    /// Testing utility that can be used to build a specific `DiceComputation` where certain keys
    /// of computation mocked to return a specific result.
    ///
    /// TODO(bobyf): ideally, we want something where we don't have to use the specific keys
    /// but rather the computation function, like `mock.expect(|c| c.other_compute(4), "4 res")`
    pub struct DiceBuilder {
        builder: DiceDataBuilder,
        mocked: Vec<Box<dyn FnOnce(&mut DiceTransactionUpdater) -> anyhow::Result<()>>>,
    }

    impl DiceBuilder {
        pub fn new() -> Self {
            let builder = Dice::builder();

            Self {
                builder,
                mocked: Vec::new(),
            }
        }

        pub fn set_data(mut self, setter: impl FnOnce(&mut DiceDataBuilder)) -> Self {
            setter(&mut self.builder);
            self
        }

        /// mocks the call of compute for the key `expected_k` so that it returns `expected_res`
        pub fn mock_and_return<K>(mut self, expected_k: K, expected_res: K::Value) -> Self
        where
            K: Key,
        {
            self.mocked.push(Box::new(move |ctx| {
                Ok(ctx.changed_to(vec![(expected_k, expected_res)])?)
            }));
            self
        }

        pub async fn build(
            self,
            extra: UserComputationData,
        ) -> anyhow::Result<DiceTransactionUpdater> {
            let dice = self.builder.build(DetectCycles::Enabled);
            let root = dice.new_root().await;
            let mut ctx = dice.updater_with_data(root, extra);

            self.mocked.into_iter().try_for_each(|f| f(&mut ctx))?;
            Ok(ctx)
        }
    }
}
