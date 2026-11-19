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
use std::ops::AsyncFnOnce;
use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use dice_error::DiceResult;
use dice_futures::cancellation::CancellationContext;
use dice_futures::owning_future::OwningFuture;
use dupe::Dupe;
use futures::FutureExt;
use futures::future::BoxFuture;
use pagable::Pagable;
use pagable::pagable_typetag;

use crate::EqualityBehavior;
use crate::InvalidationSources;
use crate::OpaqueValue;
use crate::ProjectionKey;
use crate::api::data::DiceData;
use crate::api::key::Key;
use crate::api::key::NoValueSerialize;
use crate::api::key::ValueSerialize;
use crate::api::user_data::UserComputationData;
use crate::epoch::ctx::LinearShared;
use crate::epoch::ctx::ModernDiceComputationsData;
use crate::epoch::ctx::TrackedComputations;
use crate::key::DiceKeyDyn;

/// The context for computations to register themselves, and request for additional dependencies.
/// The dependencies accessed are tracked for caching via the `DiceCtx`.
///
/// The computations are registered onto `DiceComputations` via implementing traits for the
/// `DiceComputation`.
///
/// The context is valid only for the duration of the computation of a single key, and cannot be
/// owned.
pub struct DiceComputations<'a>(pub(crate) TrackedComputations<'a>);

// SAFETY: `_assert_send_sync` destructures every field and requires it to be
// `Send + Sync`, so these impls hold exactly when the derived ones would.
//
// They are spelled out because an auto-trait proof for a future recurses into
// every type held across an await. Nearly every future in buck2 holds a
// `DiceComputations`, and without these impls each of those proofs, in each
// downstream crate, walks all of DICE's internals. Stopping the proof here
// removes a large share of downstream type-checking time.
unsafe impl Send for DiceComputations<'_> {}
unsafe impl Sync for DiceComputations<'_> {}

fn _assert_send_sync(DiceComputations(inner): &DiceComputations) {
    fn assert<T: Send + Sync>(_: &T) {}
    assert(inner);
}

impl<'d> DiceComputations<'d> {
    /// Gets the result of the given computation key.
    ///
    /// The returned reference is valid for the whole transaction, not just for the borrow of the
    /// ctx that produced it, so it can be held across later computations. Callers that need an
    /// owned value dupe it themselves.
    pub fn compute<'a, K>(
        &'a mut self,
        key: &K,
    ) -> impl Future<Output = DiceResult<&'d <K as Key>::Value>> + use<'a, 'd, K>
    where
        K: Key,
    {
        self.0.compute(key)
    }

    /// Like [`Self::compute`], but also returns the key allocation DICE
    /// retains for `key`.
    ///
    /// Equal keys resolve to the same allocation whoever requests them, so a
    /// caller that keeps references to many computed keys can hold this one
    /// instead of keeping its own copy of each key alive.
    pub fn compute_with_key<'a, K>(
        &'a mut self,
        key: &K,
    ) -> impl Future<Output = DiceResult<(Arc<K>, &'d <K as Key>::Value)>> + use<'a, 'd, K>
    where
        K: Key,
    {
        self.0.compute_with_key(key)
    }

    /// Compute "opaque" value where the value is only accessible via projections.
    /// Projections allow accessing derived results from the "opaque" value,
    /// where the dependency of reading a projection is the projection value rather
    /// than the entire opaque value.
    pub fn compute_opaque<'a, K>(
        &'a self,
        key: &K,
    ) -> impl Future<Output = DiceResult<OpaqueValue<'d, K>>> + use<'a, 'd, K>
    where
        K: Key,
    {
        self.0.compute_opaque(key)
    }

    pub fn projection<K: Key, P: ProjectionKey<DeriveFromKey = K>>(
        &mut self,
        derive_from: &OpaqueValue<K>,
        projection_key: &P,
    ) -> DiceResult<P::Value> {
        self.0.projection(derive_from, projection_key)
    }

    pub fn opaque_into_value<K: Key>(
        &mut self,
        derive_from: OpaqueValue<'d, K>,
    ) -> DiceResult<&'d K::Value> {
        Ok(self.0.opaque_into_value(derive_from))
    }

    /// DiceComputations &mut-based api can make some computations much more complex to express, but without it
    /// the data dependencies between different compute requests are impossible to track. with_linear_recompute()
    /// is an escape hatch in the case where we are willing to sacrifice recompute performance for easier expression
    /// of a computation. It should not be used lightly, it can be difficult to attribute performance regressions to its use.
    ///
    /// Withing the with_linear_recompute(), all deps will be recorded as accessed sequentially and so recomputation
    /// will not trigger them in parallel. This will apply to calls on DiceComputations returned by .ctx() and by the ones received
    /// in closures from compute_many and friends. It will not apply to the other deps from which the linear recompute was created.
    ///
    /// For example:
    ///
    /// ```ignore
    /// let mut ctx = something();
    /// let keys1 = vec![Key(10), Key(11)];
    /// let keys2 = vec![Key(20), Key(21)];
    /// let keys3 = vec![Key(30), Key(31)];
    /// let keys4 = vec![Key(40), Key(41)]
    /// ctx.compute_join(keys1, |ctx, key1| ctx.compute(key1).boxed()).await;
    /// ctx.with_linear_recompute(|mut linear| {
    ///     linear.ctx().compute_join(keys2, |ctx, key2| async move {
    ///         ctx.compute2(
    ///             |ctx| ctx.compute(key2).boxed(),
    ///             |ctx| ctx.compute_join(keys3, |ctx, key3| ctx.compute(key3).boxed()).boxed()
    ///         ).await;
    ///     }.boxed()
    /// ).await
    /// ctx.compute_join(keys4, |ctx, key4| ctx.compute(key4).boxed()).await;
    /// });
    /// ```
    ///
    /// In this example, the recomputation of all of keys2 and keys3 would be done linearly, but keys1 and keys4 would be recomputed in parallel.
    pub fn with_linear_recompute<'a, Func, T>(
        &'a mut self,
        func: Func,
    ) -> impl Future<Output = T> + use<'a, 'd, Func, T>
    where
        Func: for<'x> FnOnce(LinearRecomputeDiceComputations<'x, 'a>) -> BoxFuture<'x, T>,
    {
        self.0.with_linear_recompute(func)
    }

    /// Perform a number of computations in parallel.
    ///
    /// ## Example
    ///
    /// ```ignore
    /// let mut ctx: &'a DiceComputations = ctx();
    /// let data: String = data();
    /// let keys : Vec<Key> = keys();
    /// let futs = ctx.compute_many(keys.into_iter().map(|k|
    ///     async move |dice: &mut DiceComputations| {
    ///         dice.compute(k).await + data
    ///     }
    /// ));
    /// dice_futures::join::join_all(futs).await;
    /// ```
    ///
    /// ## Why
    ///
    /// Dice records the dependencies of a key in structured form. Concretely, that means that if
    /// you have two dependencies, `K1` and `K2`, we track whether you depended on them
    /// sequentially:
    ///
    /// ```ignore
    /// let r1 = ctx.compute(&K1).await;
    /// let r2 = ctx.compute(&K2).await;
    /// ```
    ///
    /// Or in parallel:
    ///
    /// ```ignore
    /// let (r1, r2) = ctx.compute2(&K1, &K2).await; // Also `compute_many`, `compute_join`, etc.
    /// ```
    ///
    /// To understand why, consider that the sequential case might instead look like this:
    ///
    /// ```ignore
    /// let r = ctx.compute(&K1).await;
    /// if r == 42 {
    ///     r1 += ctx.compute(&K2).await;
    /// }
    /// ```
    ///
    /// When recomputing your deps to check if they changed, we will reproduce the
    /// parallel/sequential structure. That's important because - in the example above - if the
    /// value of `K1` did indeed change away from 42, initiating evaluation of `K2` may be
    /// inappropriate. At the very least it's wasted work, but it can violate preconditions for
    /// evaluating K2 (though we discourage such preconditions anyway, but they're not always easy
    /// to avoid).
    ///
    /// So: This API. You can still ask for things to be done in parallel, but must do so in a way
    /// that dice can understand.
    ///
    /// If you do not want to put up with this, you have an escape hatch in the form of
    /// `DiceComputations::with_linear_recompute`. `with_linear_recompute` gives you free access to
    /// unstructured dependencies, in exchange for pessimizing the recompute path by doing it in
    /// some linearized order.
    pub fn compute_many<'a, Computes, F, T>(
        &'a mut self,
        computes: Computes,
        // Note that both here and elsewhere in this file the `Send` being explicit is load-bearing;
        // without it, the compiler struggles to find the right strategy for proving the bound later
        // on.
    ) -> Vec<impl Future<Output = T> + Send + use<'a, 'd, Computes, F, T>>
    where
        Computes: IntoIterator<Item = F>,
        Computes::IntoIter: ExactSizeIterator,
        F: for<'x> AsyncFnOnce(&'x mut DiceComputations<'d>) -> T + Send,
        // There's some very funky things going on here. This trait impl here, as written, works and
        // does the expected thing. However, if you doing this exact same thing with `FnOnce`
        // instead of `AsyncFnOnce`, the compiler has a self-admitted bug where it forces callers
        // into `'d = 'static`. Why `AsyncFnOnce` is different in any regard remains a bit of a
        // mystery, my suspicion is that it's because of the `Self` in `CallOnceFuture: ...Self...`.
        //
        // Interestingly enough, that is exactly the boat in which
        // `TrackedComputations::compute_many` finds itself. It works around this by taking
        // `FnOnce(&'a mut DiceC...` instead of `for<'x> FnOnce(&'x ...`. That makes for a mildly
        // less sound API but avoids a bunch of the pain.
        //
        // If any of this turns into a rustc upgrade hazard at any point, ie there are changes to
        // some of the APIs that are not just cosmetic and can't be easily addressed then, in
        // decreasing order of preference:
        //
        //  - Propagate the signature of this API down into `TrackedComputations`
        //  - Wrap some things in an `UnsafeSendAssert` and comment out/weaken send bounds as needed
        //    to unblock.
        //  - Propagate the signature of the `TrackedComputations` API back here, removing the async
        //    closures
        for<'x> <F as AsyncFnOnce<(&'x mut DiceComputations<'d>,)>>::CallOnceFuture: Send,
    {
        self.0.compute_many(
            computes
                .into_iter()
                .map(|f| move |ctx: &'a mut DiceComputations<'d>| f(ctx)),
        )
    }

    /// Maps the items into computation futures and joins on them.
    ///
    /// ```ignore
    /// let mut ctx: &'a DiceComputations = ctx();
    /// let data: String = data();
    /// let keys : Vec<Key> = keys();
    /// ctx.compute_join(keys, |dice: &mut DiceComputations, k: &Key| {
    ///     async move {
    ///       dice.compute(k).await + data
    ///     }
    ///   }).await;
    /// ```
    pub fn compute_join<'a, Items, Mapper, T, R>(
        &'a mut self,
        items: Items,
        mapper: Mapper,
    ) -> impl Future<Output = Vec<R>> + Send + use<'a, 'd, Items, Mapper, T, R>
    where
        Items: IntoIterator<Item = T>,
        Items::IntoIter: ExactSizeIterator,
        Mapper: for<'x> AsyncFnOnce(&'x mut DiceComputations<'d>, T) -> R + Send + Sync + Copy,
        for<'x> <Mapper as AsyncFnOnce<(&'x mut DiceComputations<'d>, T)>>::CallOnceFuture: Send,
        T: Send,
        R: Send,
    {
        let futs = self.0.compute_many(
            items
                .into_iter()
                .map(|v| move |ctx: &'a mut DiceComputations<'d>| mapper(ctx, v)),
        );
        // We embed the `unconstrained` here because buck2 has always benefitted from this
        tokio::task::unconstrained(dice_futures::join::join_all(futs))
    }

    /// Maps the items into computations futures and then returns a future which represents either a
    /// collection of the results or an error.
    pub fn try_compute_join<'a, Items, Mapper, T, R, E>(
        &'a mut self,
        items: Items,
        mapper: Mapper,
    ) -> impl Future<Output = Result<Vec<R>, E>> + Send + use<'a, 'd, Items, Mapper, T, R, E>
    where
        Items: IntoIterator<Item = T>,
        Items::IntoIter: ExactSizeIterator,
        Mapper: for<'x> AsyncFnOnce(&'x mut DiceComputations<'d>, T) -> Result<R, E>
            + Send
            + Sync
            + Copy,
        for<'x> <Mapper as AsyncFnOnce<(&'x mut DiceComputations<'d>, T)>>::CallOnceFuture: Send,
        T: Send,
        R: Send,
        E: Send,
    {
        let futs = self.0.compute_many(
            items
                .into_iter()
                .map(|v| move |ctx: &'a mut DiceComputations<'d>| mapper(ctx, v)),
        );
        tokio::task::unconstrained(dice_futures::join::try_join_all(futs))
    }

    /// Computes all the given tasks in parallel.
    pub fn compute2<'a, Compute1, T, Compute2, U>(
        &'a mut self,
        compute1: Compute1,
        compute2: Compute2,
    ) -> impl Future<Output = (T, U)> + Send + use<'a, 'd, Compute1, T, Compute2, U>
    where
        Compute1: for<'x> AsyncFnOnce(&'x mut DiceComputations<'d>) -> T + Send,
        for<'x> <Compute1 as AsyncFnOnce<(&'x mut DiceComputations<'d>,)>>::CallOnceFuture: Send,
        Compute2: for<'x> AsyncFnOnce(&'x mut DiceComputations<'d>) -> U + Send,
        for<'x> <Compute2 as AsyncFnOnce<(&'x mut DiceComputations<'d>,)>>::CallOnceFuture: Send,
        T: Send,
        U: Send,
    {
        let (t, u) = self
            .0
            .compute2(move |ctx| compute1(ctx), move |ctx| compute2(ctx));
        futures::future::join(t, u)
    }

    /// Like [`Self::compute2`], but the computations can fail, returning an error if any of them do.
    pub fn try_compute2<'a, Compute1, T, Compute2, U, E>(
        &'a mut self,
        compute1: Compute1,
        compute2: Compute2,
    ) -> impl Future<Output = Result<(T, U), E>> + Send + use<'a, 'd, Compute1, T, Compute2, U, E>
    where
        Compute1: for<'x> AsyncFnOnce(&'x mut DiceComputations<'d>) -> Result<T, E> + Send,
        for<'x> <Compute1 as AsyncFnOnce<(&'x mut DiceComputations<'d>,)>>::CallOnceFuture: Send,
        Compute2: for<'x> AsyncFnOnce(&'x mut DiceComputations<'d>) -> Result<U, E> + Send,
        for<'x> <Compute2 as AsyncFnOnce<(&'x mut DiceComputations<'d>,)>>::CallOnceFuture: Send,
        T: Send,
        U: Send,
        E: Send,
    {
        let (t, u) = self
            .0
            .compute2(move |ctx| compute1(ctx), move |ctx| compute2(ctx));
        futures::future::try_join(t, u)
    }

    /// Like [`Self::compute2`], but with three computations.
    pub fn compute3<'a, Compute1, T, Compute2, U, Compute3, V>(
        &'a mut self,
        compute1: Compute1,
        compute2: Compute2,
        compute3: Compute3,
    ) -> impl Future<Output = (T, U, V)> + Send + use<'a, 'd, Compute1, T, Compute2, U, Compute3, V>
    where
        Compute1: for<'x> AsyncFnOnce(&'x mut DiceComputations<'d>) -> T + Send,
        for<'x> <Compute1 as AsyncFnOnce<(&'x mut DiceComputations<'d>,)>>::CallOnceFuture: Send,
        Compute2: for<'x> AsyncFnOnce(&'x mut DiceComputations<'d>) -> U + Send,
        for<'x> <Compute2 as AsyncFnOnce<(&'x mut DiceComputations<'d>,)>>::CallOnceFuture: Send,
        Compute3: for<'x> AsyncFnOnce(&'x mut DiceComputations<'d>) -> V + Send,
        for<'x> <Compute3 as AsyncFnOnce<(&'x mut DiceComputations<'d>,)>>::CallOnceFuture: Send,
        T: Send,
        U: Send,
        V: Send,
    {
        let (t, u, v) = self.0.compute3(
            move |ctx| compute1(ctx),
            move |ctx| compute2(ctx),
            move |ctx| compute3(ctx),
        );
        futures::future::join3(t, u, v)
    }

    /// Like [`Self::compute3`], but the computations can fail, returning an error if any of them do.
    pub fn try_compute3<'a, Compute1, T, Compute2, U, Compute3, V, E>(
        &'a mut self,
        compute1: Compute1,
        compute2: Compute2,
        compute3: Compute3,
    ) -> impl Future<Output = Result<(T, U, V), E>>
    + Send
    + use<'a, 'd, Compute1, T, Compute2, U, Compute3, V, E>
    where
        Compute1: for<'x> AsyncFnOnce(&'x mut DiceComputations<'d>) -> Result<T, E> + Send,
        for<'x> <Compute1 as AsyncFnOnce<(&'x mut DiceComputations<'d>,)>>::CallOnceFuture: Send,
        Compute2: for<'x> AsyncFnOnce(&'x mut DiceComputations<'d>) -> Result<U, E> + Send,
        for<'x> <Compute2 as AsyncFnOnce<(&'x mut DiceComputations<'d>,)>>::CallOnceFuture: Send,
        Compute3: for<'x> AsyncFnOnce(&'x mut DiceComputations<'d>) -> Result<V, E> + Send,
        for<'x> <Compute3 as AsyncFnOnce<(&'x mut DiceComputations<'d>,)>>::CallOnceFuture: Send,
        T: Send,
        U: Send,
        V: Send,
        E: Send,
    {
        let (t, u, v) = self.0.compute3(
            move |ctx| compute1(ctx),
            move |ctx| compute2(ctx),
            move |ctx| compute3(ctx),
        );
        futures::future::try_join3(t, u, v)
    }

    /// Used to declare a higher order closure for compute_join and try_compute_join.
    ///
    /// A closure that is created where a compute_join-shaped closure is expected infers its
    /// signature from that expected type and needs no help. This helper provides that same
    /// expected type for closures declared at a distance - stored in a variable, accumulated in
    /// a `Vec`, or created inside an iterator adapter for `compute_many` - where the compiler
    /// would otherwise have nothing to infer the ctx type from.
    pub fn declare_join_closure<'a, T, R, Closure>(closure: Closure) -> Closure
    where
        'd: 'a,
        Closure: FnOnce(&'a mut DiceComputations<'d>, T) -> BoxFuture<'a, R> + Send + Sync + Copy,
    {
        closure
    }

    /// Used to declare a higher order closure for compute2 and compute_many.
    ///
    /// See `declare_join_closure` for when this is needed.
    pub fn declare_closure<Closure, R>(closure: Closure) -> Closure
    where
        Closure: for<'x> AsyncFnOnce(&'x mut DiceComputations<'d>) -> R + Send,
        for<'x> <Closure as AsyncFnOnce<(&'x mut DiceComputations<'d>,)>>::CallOnceFuture: Send,
    {
        closure
    }

    /// Returns a handle to the DICE data that provides access to global and per-transaction
    /// data without the ability to request new keys.
    pub fn data(&self) -> DiceComputationsData {
        self.0.data()
    }

    /// Untracked data that is static for the lifetime of this branch tree.
    pub fn global_data(&self) -> &'d DiceData {
        self.0.global_data()
    }

    /// Data that is static for the lifetime of the current request context. This lifetime is
    /// the lifetime of the top-level `DiceComputation` used for all requests.
    /// The data is also specific to each request context, so multiple concurrent requests can
    /// each have their own individual data.
    pub fn per_transaction_data(&self) -> &'d UserComputationData {
        self.0.per_transaction_data()
    }

    /// Store some extra data that the ActivationTracker will receive if / when this key finishes
    /// executing.
    pub fn store_evaluation_data<T: Send + Sync + 'static>(&self, value: T) -> DiceResult<()> {
        self.0.store_evaluation_data(value)
    }

    /// The invalidation sources of the dependencies requested so far.
    pub fn get_invalidation_sources(&mut self) -> InvalidationSources {
        self.0.get_invalidation_sources()
    }

    /// Spawn a computation on a new tokio task.
    ///
    /// This allows spawning work that requires access to the `DiceComputations` context
    /// without running into lifetime issues with `tokio::spawn` requiring `'static` futures.
    /// Dependencies accessed in the spawned task are tracked and merged back into the
    /// parent computation. If the returned future is dropped, the spawned task will be cancelled.
    pub fn spawned<'a, T, Compute>(
        &'a mut self,
        closure: Compute,
    ) -> impl Future<Output = T> + use<'a, 'd, Compute, T>
    where
        T: Send + 'static,
        Compute: (for<'x> FnOnce(
                &'x mut DiceComputations<'_>,
                &'x CancellationContext,
            ) -> BoxFuture<'x, T>)
            + Send
            + 'static,
    {
        self.0.spawned(closure)
    }
}

#[derive(Copy, Clone, Dupe)]
pub struct LinearRecomputeDiceComputations<'l, 'a>(pub(crate) &'l LinearShared<'a>);

// SAFETY: as for `DiceComputations`.
unsafe impl Send for LinearRecomputeDiceComputations<'_, '_> {}
unsafe impl Sync for LinearRecomputeDiceComputations<'_, '_> {}

fn _assert_linear_send_sync(
    LinearRecomputeDiceComputations(inner): &LinearRecomputeDiceComputations,
) {
    fn assert<T: Send + Sync>(_: &T) {}
    assert(inner);
}

impl<'l, 'a> LinearRecomputeDiceComputations<'l, 'a> {
    pub fn get(&self) -> DiceComputations<'l> {
        self.0.get()
    }

    /// Spawn a computation on a new tokio task.
    ///
    /// See [`DiceComputations::spawned`] for details.
    pub fn spawned<T, Compute>(
        &self,
        closure: Compute,
    ) -> impl Future<Output = T> + use<'a, 'l, Compute, T>
    where
        T: Send + 'static,
        Compute: (for<'x> FnOnce(
                &'x mut DiceComputations<'_>,
                &'x CancellationContext,
            ) -> BoxFuture<'x, T>)
            + Send
            + 'static,
    {
        OwningFuture::new(self.get(), |ctx| ctx.0.spawned(closure).boxed())
    }
}

/// A holder for the user data attached to DICE. APIs that require access to data stored on DICE
/// but that won't request keys can indicate that by accepting a `DiceComputationsData`.
#[derive(Clone, Dupe)]
pub struct DiceComputationsData(pub(crate) ModernDiceComputationsData);

impl DiceComputationsData {
    /// Untracked data that is static for the lifetime of this branch tree.
    pub fn global_data(&self) -> &DiceData {
        self.0.global_data()
    }

    /// Data that is static for the lifetime of the current request context. This lifetime is
    /// the lifetime of the top-level `DiceComputation` used for all requests.
    /// The data is also specific to each request context, so multiple concurrent requests can
    /// each have their own individual data.
    pub fn per_transaction_data(&self) -> &UserComputationData {
        self.0.per_transaction_data()
    }
}

// This assertion assures we don't unknowingly regress the size of this critical future.
// TODO(cjhopman): We should be able to wrap this in a convenient assertion macro.
#[allow(unused, clippy::diverging_sub_expression)]
fn _assert_dice_compute_future_sizes() {
    #[derive(Allocative, Debug, Clone, PartialEq, Eq, Hash, Pagable)]
    #[pagable_typetag(DiceKeyDyn)]
    struct K(u64);
    impl std::fmt::Display for K {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!()
        }
    }

    #[async_trait]
    impl Key for K {
        type Value = Arc<String>;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            cancellations: &CancellationContext,
        ) -> Self::Value {
            panic!()
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| panic!())
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    mini_vec::size_assert::words_of_async_fn_future!(DiceComputations::compute::<K>, (_, _), 9);

    // The future of the canonical async closure mapper - the thing that is stored, unboxed, in the
    // join combinator's slot for each branch of a `compute_join_async`. This is mostly just here to
    // detect rustc size regressions (or improvements)
    //
    // The transaction lifetime is spelled `'static` because a closure written out standalone like
    // this doesn't get the implied `'d: '_` bound that the real `compute_join` bounds provide, and
    // without it returning the borrowed value doesn't typecheck. Lifetimes don't affect the size.
    mini_vec::size_assert::words_of_expr!(
        (async |ctx: &mut DiceComputations<'static>, k: &K| ctx.compute(k).await)
            .async_call_once((panic!(), panic!())),
        12
    );

    mini_vec::size_assert::words_of_async_fn_future!(
        DiceComputations::compute_join,
        (
            _,
            Vec::<u32>::new(),
            async |ctx: &mut DiceComputations, _k: u32| -> u32 { panic!() }
        ),
        3
    );

    // Demonstrate that these are inline
    mini_vec::size_assert::words_of_async_fn_future!(
        DiceComputations::compute2,
        (
            _,
            async |ctx| ctx.compute(&K(0)).await,
            async |ctx| ctx.compute(&K(1)).await
        ),
        26
    );
}
