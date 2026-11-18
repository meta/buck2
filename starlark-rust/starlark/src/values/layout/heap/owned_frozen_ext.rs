/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Conveniences on [`OwnedFrozen`] and [`OwnedFrozenRef`], built on the safe API of
//! `owned_frozen.rs`: nothing here reaches the pairing of value and heap.

use std::fmt;

use allocative::Allocative;
use dupe::Dupe;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;

use crate::any::IsStaticType;
use crate::pagable::StarlarkSerialize;
use crate::pagable::starlark_deserialize::StarlarkDeserializeContext;
use crate::pagable::starlark_deserialize_context::StarlarkDeserializerImpl;
use crate::pagable::starlark_serialize_context::StarlarkSerializerImpl;
use crate::values::Deferred;
use crate::values::DeferredWord;
use crate::values::HeapSendable;
use crate::values::HeapSyncable;
use crate::values::OwnedFrozen;
use crate::values::OwnedFrozenRef;
use crate::values::StarlarkTypeExpr;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::layout::heap::sealed::FrozenHeapArc;

/// An alias for `FnOnce`.
///
/// `FnOncish<T, U>` should just be read as `FnOnce(T) -> U`.
///
/// This has to exist to work around a limitation in the type system:
/// <https://github.com/rust-lang/rust/issues/49601>.
pub trait FnOncish<T, U>: FnOnce(T) -> U {}

impl<F, T, U> FnOncish<T, U> for F where F: FnOnce(T) -> U {}

/// See [`FnOncish`].
pub trait FnOncish2<T1, T2, U>: FnOnce(T1, T2) -> U {}

impl<F, T1, T2, U> FnOncish2<T1, T2, U> for F where F: FnOnce(T1, T2) -> U {}

impl<T: IsStaticType> OwnedFrozen<T>
where
    for<'fv> T::Reinfect<'fv>: Sized,
{
    /// Access the underlying value in a closure
    pub fn by_ref<'s, F, R>(&'s self, f: F) -> R
    where
        for<'a, 'fv> F: FnOnce(&'a T::Reinfect<'fv>) -> R,
    {
        self.by_ref_with_reconstructor(|v, _r| f(v))
    }

    /// Transform the contained value
    pub fn map<U, F>(self, f: F) -> OwnedFrozen<U>
    where
        U: IsStaticType,
        for<'fv> U::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv> + Sized,
        for<'fv> F: FnOncish<T::Reinfect<'fv>, U::Reinfect<'fv>>,
    {
        match self.try_map::<_, std::convert::Infallible, _>(|v| Ok(f(v))) {
            Ok(x) => x,
        }
    }

    /// Transform the contained value
    pub fn try_map<U, E, F>(self, f: F) -> Result<OwnedFrozen<U>, E>
    where
        U: IsStaticType,
        for<'fv> U::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv> + Sized,
        for<'fv> F: FnOncish<T::Reinfect<'fv>, Result<U::Reinfect<'fv>, E>>,
    {
        self.try_by_value_with_reconstructor(|v, _r| (f(v), ())).0
    }

    /// Transform the contained value
    pub fn maybe_map<U, F>(self, f: F) -> Option<OwnedFrozen<U>>
    where
        U: IsStaticType,
        for<'fv> U::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv> + Sized,
        for<'fv> F: FnOncish<T::Reinfect<'fv>, Option<U::Reinfect<'fv>>>,
    {
        self.try_map(|v| f(v).ok_or(())).ok()
    }
}

impl OwnedFrozen<Value<'static>> {
    /// Check that the value is a `T`, returning an error describing the actual type if not.
    pub fn downcast_starlark<T: IsStaticType + StarlarkValue<'static>>(
        self,
    ) -> crate::Result<OwnedFrozen<ValueTyped<'static, T>>>
    where
        for<'fv> T::Reinfect<'fv>: StarlarkValue<'fv> + Sized,
        for<'fv> ValueTyped<'fv, T::Reinfect<'fv>>: HeapSendable<'fv> + HeapSyncable<'fv>,
    {
        self.try_map::<ValueTyped<'static, T>, crate::Error, _>(|v| ValueTyped::new_err(v))
    }
}

impl<'f, T: IsStaticType> OwnedFrozenRef<'f, T>
where
    for<'fv> T::Reinfect<'fv>: Sized,
{
    /// Transform the contained value
    pub fn map<U, F>(self, f: F) -> OwnedFrozenRef<'f, U>
    where
        U: IsStaticType,
        for<'fv> U::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv> + Sized,
        for<'fv> F: FnOncish<T::Reinfect<'fv>, U::Reinfect<'fv>>,
    {
        match self.try_map::<_, std::convert::Infallible, _>(|v| Ok(f(v))) {
            Ok(x) => x,
        }
    }

    /// Transform the contained value
    pub fn maybe_map<U, F>(self, f: F) -> Option<OwnedFrozenRef<'f, U>>
    where
        U: IsStaticType,
        for<'fv> U::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv> + Sized,
        for<'fv> F: FnOncish<T::Reinfect<'fv>, Option<U::Reinfect<'fv>>>,
    {
        self.try_map(|v| f(v).ok_or(())).ok()
    }
}

impl<T: IsStaticType> fmt::Debug for OwnedFrozen<T>
where
    for<'fv> T::Reinfect<'fv>: Sized,
    for<'fv> T::Reinfect<'fv>: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.by_ref(|v| fmt::Debug::fmt(v, f))
    }
}

impl<T: IsStaticType> fmt::Display for OwnedFrozen<T>
where
    for<'fv> T::Reinfect<'fv>: Sized,
    for<'fv> T::Reinfect<'fv>: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.by_ref(|v| fmt::Display::fmt(v, f))
    }
}

impl<T: IsStaticType> Clone for OwnedFrozen<T>
where
    for<'fv> T::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv> + Clone + Sized,
{
    fn clone(&self) -> Self {
        self.by_ref_with_reconstructor(|v, r| r.reconstruct(v.clone()))
    }
}

impl<T: IsStaticType> Dupe for OwnedFrozen<T> where
    for<'fv> T::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv> + Dupe + Sized
{
}

impl<T: IsStaticType> Allocative for OwnedFrozen<T>
where
    for<'fv> T::Reinfect<'fv>: Sized,
    for<'fv> T::Reinfect<'fv>: Allocative,
{
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        visitor.visit_field(allocative::Key::new("owner"), self.heap_arc());
        self.by_ref(|v| v.visit(&mut visitor));
        visitor.exit();
    }
}

impl<T> std::ops::Deref for OwnedFrozen<T>
where
    for<'fv> T: IsStaticType<Reinfect<'fv> = T>,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.get()
    }
}

/// The wire format for every `OwnedFrozen` is the owner heap followed by the frozen value.
///
/// It is shared by the `Value`, `ValueTyped` and `ValueOf` forms so they can be swapped at a
/// field without a format change.
fn serialize_owned_frozen(
    owner: &FrozenHeapArc,
    value: &impl StarlarkSerialize,
    serializer: &mut dyn PagableSerializer,
) -> pagable::Result<()> {
    // Serialize the owner heap (via pagable arc mechanism).
    owner.pagable_serialize(serializer)?;

    // Ensure offset maps are registered for the owner heap and its transitive dependencies.
    // `serialize_arc` for `Arc<FrozenFrozenHeap>` can defer the actual heap serialization, so
    // the offset maps may not exist yet when we need to serialize the value.
    let state = StarlarkSerializerImpl::get_or_create_state(serializer);
    state.ensure_chunk_index_registered(owner)?;

    let mut ctx = StarlarkSerializerImpl::new_with_root(serializer, state, owner);
    value
        .starlark_serialize(&mut ctx)
        .map_err(|e| e.into_anyhow())?;

    Ok(())
}

/// See [`serialize_owned_frozen`].
fn deserialize_owned_frozen<'de, D: PagableDeserializer<'de> + ?Sized>(
    deserializer: &mut D,
) -> pagable::Result<OwnedFrozen<Value<'static>>> {
    // Deserialize the owner heap.
    let owner = OwnedFrozen::<()>::pagable_deserialize(deserializer)?;

    // The preceding owner heap registered itself in the Starlark scope, so
    // cross-heap pointer resolution can find it.
    let origin = owner.heap_arc().dupe();
    StarlarkDeserializerImpl::recover_root_from_pagable_in(deserializer.as_dyn(), &origin, |ctx| {
        let value = ctx.deserialize_value().map_err(|e| e.into_anyhow())?;
        // SAFETY: The context's brand is `owner`'s heap, which the value was resolved against,
        // so `owner` keeps it alive.
        Ok(unsafe { OwnedFrozen::unchecked_new(owner, value) })
    })
}

impl PagableSerialize for OwnedFrozen<Value<'static>> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        self.by_ref(|v| serialize_owned_frozen(self.heap_arc(), v, serializer))
    }
}

/// A deferred value kept alive by its owning frozen heap.
///
/// [`new`](Self::new) retains an already available owned value.
/// [`read`](Self::read) returns an owner-preserving borrowed view, materializing
/// the value if necessary; [`peek`](Self::peek) never materializes it.
/// [`by_ref`](Self::by_ref) reads before invoking a brand-generic closure.
/// `Debug` and memory accounting do not materialize the value.
///
/// The owner is retained from construction or deserialization until this
/// wrapper is dropped. Deserialization is eager by default and can be deferred
/// by the storage policy. Unread values require their storage to remain open.
pub struct DeferredOwnedFrozen<T: DeferredWord + IsStaticType> {
    owner: OwnedFrozen<()>,
    value: Deferred<T>,
}

// SAFETY: as for `OwnedFrozen` (see the note on its impls): the value is kept alive by `owner`,
// and the `HeapSendable`/`HeapSyncable` bounds that make that sound are imposed at construction,
// where the compiler can prove them.
unsafe impl<T: DeferredWord + IsStaticType> Send for DeferredOwnedFrozen<T> {}
unsafe impl<T: DeferredWord + IsStaticType> Sync for DeferredOwnedFrozen<T> {}

impl<T> DeferredOwnedFrozen<T>
where
    T: DeferredWord + IsStaticType + Copy,
    for<'fv> T::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv> + Sized,
{
    /// Constructs a resident field, retaining the existing value's heap owner.
    pub fn new(owned: OwnedFrozen<T>) -> Self {
        // SAFETY: both parts are stored together in the returned wrapper, which only
        // hands the value out through `owner`.
        let (owner, value) = unsafe { owned.into_parts() };
        Self {
            owner,
            value: Deferred::new(value),
        }
    }

    /// Returns an owner-preserving borrowed view, materializing the value if needed.
    /// May block, and fails as [`Deferred::read`] does.
    pub fn read(&self) -> crate::Result<OwnedFrozenRef<'_, T>> {
        let value = *self.value.read()?;
        // SAFETY: `value` was erased from `owner`'s heap by `into_parts`, or resolved against it.
        Ok(unsafe { OwnedFrozenRef::from_erased(self.owner.owner(), value) })
    }

    /// Returns a borrowed view if available, or `None` if pending or being resolved.
    /// Never materializes the value or waits for resolution.
    pub fn peek(&self) -> Option<OwnedFrozenRef<'_, T>> {
        let value = *self.value.peek()?;
        // SAFETY: as in `read`.
        Some(unsafe { OwnedFrozenRef::from_erased(self.owner.owner(), value) })
    }

    /// Reads the value and invokes a closure generic over its heap brand.
    /// May block, and fails as [`read`](Self::read) does.
    /// Values borrowing from the closure's argument cannot escape the closure.
    ///
    /// ```compile_fail
    /// use starlark::values::{DeferredOwnedFrozen, Value};
    ///
    /// fn escape(value: &DeferredOwnedFrozen<Value<'static>>) -> Value<'static> {
    ///     value.by_ref(|v| *v).unwrap()
    /// }
    /// ```
    pub fn by_ref<F, R>(&self, f: F) -> crate::Result<R>
    where
        for<'a, 'fv> F: FnOnce(&'a T::Reinfect<'fv>) -> R,
    {
        Ok(f(&self.read()?.value()))
    }

    /// As [`by_ref`](Self::by_ref), for a closure that can itself fail.
    pub fn try_by_ref<F, R, E>(&self, f: F) -> Result<R, E>
    where
        for<'a, 'fv> F: FnOnce(&'a T::Reinfect<'fv>) -> Result<R, E>,
        E: From<crate::Error>,
    {
        f(&self.read()?.value())
    }

    /// The heap the value lives in.
    pub fn owner(&self) -> OwnedFrozenRef<'_, ()> {
        self.owner.owner()
    }
}

impl<T: DeferredWord + IsStaticType + fmt::Debug> fmt::Debug for DeferredOwnedFrozen<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.value, f)
    }
}

impl<T: DeferredWord + IsStaticType + Allocative> Allocative for DeferredOwnedFrozen<T> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        visitor.visit_field(allocative::Key::new("owner"), &self.owner);
        visitor.visit_field(allocative::Key::new("value"), &self.value);
        visitor.exit();
    }
}

impl PagableSerialize for DeferredOwnedFrozen<Value<'static>> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        serialize_owned_frozen(self.owner.heap_arc(), &self.value, serializer)
    }
}

impl<'de> PagableDeserialize<'de> for DeferredOwnedFrozen<Value<'static>> {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        deserialize_deferred_owned_frozen(deserializer)
    }
}

impl<T: IsStaticType + StarlarkValue<'static>> PagableSerialize
    for DeferredOwnedFrozen<ValueTyped<'static, T>>
where
    for<'fv> T::Reinfect<'fv>: StarlarkValue<'fv> + Sized,
    for<'fv> ValueTyped<'fv, T::Reinfect<'fv>>: HeapSendable<'fv> + HeapSyncable<'fv>,
{
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        serialize_owned_frozen(self.owner.heap_arc(), &self.value, serializer)
    }
}

impl<'de, T: IsStaticType + StarlarkValue<'static>> PagableDeserialize<'de>
    for DeferredOwnedFrozen<ValueTyped<'static, T>>
where
    for<'fv> T::Reinfect<'fv>: StarlarkValue<'fv> + Sized,
    for<'fv> ValueTyped<'fv, T::Reinfect<'fv>>: HeapSendable<'fv> + HeapSyncable<'fv>,
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        deserialize_deferred_owned_frozen(deserializer)
    }
}

fn deserialize_deferred_owned_frozen<'de, T, D>(
    deserializer: &mut D,
) -> pagable::Result<DeferredOwnedFrozen<T>>
where
    T: DeferredWord + IsStaticType + Copy,
    for<'fv> T::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv> + Sized,
    D: PagableDeserializer<'de> + ?Sized,
{
    let owner = OwnedFrozen::<()>::pagable_deserialize(deserializer)?;
    let origin = owner.heap_arc().dupe();
    StarlarkDeserializerImpl::recover_root_from_pagable_in(deserializer.as_dyn(), &origin, |ctx| {
        // SAFETY: `owner` is the exact heap of `ctx`. The erased field stays
        // private in the owning wrapper and every public access restores a brand.
        let value = unsafe { Deferred::<T>::deserialize_owned(ctx) }
            .map_err(|error| error.into_anyhow())?;
        Ok(DeferredOwnedFrozen { owner, value })
    })
}

impl<'de> PagableDeserialize<'de> for OwnedFrozen<Value<'static>> {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        deserialize_owned_frozen(deserializer)
    }
}

impl<T: IsStaticType + StarlarkValue<'static>> PagableSerialize
    for OwnedFrozen<ValueTyped<'static, T>>
where
    for<'fv> T::Reinfect<'fv>: StarlarkValue<'fv> + Sized,
{
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        self.by_ref(|v| serialize_owned_frozen(self.heap_arc(), v, serializer))
    }
}

impl<'de, T: IsStaticType + StarlarkValue<'static>> PagableDeserialize<'de>
    for OwnedFrozen<ValueTyped<'static, T>>
where
    for<'fv> T::Reinfect<'fv>: StarlarkValue<'fv> + Sized,
    for<'fv> ValueTyped<'fv, T::Reinfect<'fv>>: HeapSendable<'fv> + HeapSyncable<'fv>,
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        deserialize_owned_frozen(deserializer)?
            .downcast_starlark::<T>()
            .map_err(|e| anyhow::anyhow!("OwnedFrozen deserialization: {e}"))
    }
}

impl<T> PagableSerialize for OwnedFrozen<ValueOf<'static, T>>
where
    T: StarlarkTypeExpr<'static> + IsStaticType,
    for<'fv> T::Reinfect<'fv>: StarlarkTypeExpr<'fv> + Sized,
{
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        self.by_ref(|v| serialize_owned_frozen(self.heap_arc(), v, serializer))
    }
}

impl<'de, T> PagableDeserialize<'de> for OwnedFrozen<ValueOf<'static, T>>
where
    T: StarlarkTypeExpr<'static> + IsStaticType,
    for<'fv> T::Reinfect<'fv>: StarlarkTypeExpr<'fv> + Sized,
    for<'fv> ValueOf<'fv, T::Reinfect<'fv>>: HeapSendable<'fv> + HeapSyncable<'fv>,
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        deserialize_owned_frozen(deserializer)?
            .try_map::<ValueOf<'static, T>, crate::Error, _>(|v| ValueOf::new_err(v))
            .map_err(|e| anyhow::anyhow!("OwnedFrozen deserialization: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use allocative::Allocative;
    use derive_more::Display;
    use starlark_derive::Freeze;
    use starlark_derive::NoSerialize;
    use starlark_derive::Trace;
    use starlark_derive::starlark_value;

    use crate as starlark;
    use crate::any::ProvidesStaticType;
    use crate::starlark_complex_value;
    use crate::values::DeferredOwnedFrozen;
    use crate::values::OwnedFrozen;
    use crate::values::StarlarkValue;
    use crate::values::Value;
    use crate::values::ValueTyped;
    use crate::values::layout::heap::name::StarlarkTestHeapName;

    #[allow(dead_code)]
    fn construct_any<T>() -> T {
        unreachable!()
    }

    fn _check_send_sync()
    where
        OwnedFrozen<Value<'static>>: Send + Sync,
    {
    }

    fn _check_send_sync_provable_in_generator_interior() {
        // An async block holding a `OwnedFrozen<Value<'static>>` in a capture
        //
        // When attempting to prove that this future is `Send`, rustc incorrectly tries to prove
        // `for<'fv> OwnedFrozen<Value<'fv>>: Send` instead of just the `'static` case. This is
        // the standard mcve for <https://github.com/rust-lang/rust/issues/102211>.
        async fn hold_in_generator_interior() {
            let v: OwnedFrozen<Value<'static>> = construct_any();
            async {}.await;
            drop(v);
        }

        fn _prove_send_sync() -> impl Send + Sync {
            hold_in_generator_interior()
        }
    }

    #[derive(
        Clone,
        Debug,
        Display,
        Trace,
        Freeze,
        ProvidesStaticType,
        NoSerialize,
        Allocative,
        starlark_derive::StarlarkPagable
    )]
    struct MyComplex<'v>(Value<'v>);

    starlark_complex_value!(MyComplex);

    #[starlark_value(type = "MyComplex")]
    impl<'v> StarlarkValue<'v> for MyComplex<'v> {}

    fn _check_downcast_starlark_actually_usable() {
        let v: OwnedFrozen<Value<'static>> = construct_any();
        let _v: OwnedFrozen<ValueTyped<'static, MyComplex<'static>>> =
            v.downcast_starlark::<MyComplex<'static>>().unwrap();
    }

    #[test]
    fn test_owned_frozen_ref() {
        let owned: OwnedFrozen<Value<'static>> = OwnedFrozen::build(
            crate::values::layout::heap::name::StarlarkTestHeapName::frozen_heap_name(),
            |heap| heap.alloc("contents").to_value(),
        );

        let r = owned.as_ref();
        assert_eq!(r.value().unpack_str(), Some("contents"));
        assert!(r.owner() == owned.owner());

        let r = r
            .maybe_map::<Value<'static>, _>(|v| Some(v))
            .unwrap()
            .map::<Value<'static>, _>(|v| v);
        let owned2 = r.to_owned();
        owned2.by_ref(|v| assert_eq!(v.unpack_str(), Some("contents")));

        crate::values::Heap::temp(|unfrozen| {
            let v = owned.as_ref().add_to_heap(unfrozen);
            assert_eq!(v.unpack_str(), Some("contents"));
        });

        crate::values::Heap::temp(|unfrozen| {
            let v = owned.by_ref_with_reconstructor(|v, r| r.edge(unfrozen).rebrand(*v));
            assert_eq!(v.unpack_str(), Some("contents"));
        });

        crate::values::FrozenHeap::temp(|other| {
            let v = owned.as_ref().add_to_frozen_heap(other);
            assert_eq!(v.unpack_str(), Some("contents"));
        });
    }

    #[test]
    fn test_deferred_owned_frozen_resident() {
        let owned: OwnedFrozen<Value<'static>> =
            OwnedFrozen::build(StarlarkTestHeapName::frozen_heap_name(), |heap| {
                heap.alloc("contents").to_value()
            });
        let owner = owned.owner().to_owned();
        let deferred = DeferredOwnedFrozen::new(owned);

        assert!(deferred.owner() == owner.owner());
        assert_eq!(
            deferred.read().unwrap().value().unpack_str(),
            Some("contents")
        );
        assert_eq!(
            deferred.peek().unwrap().value().unpack_str(),
            Some("contents"),
            "a resident value is resolved without a read"
        );
        assert_eq!(
            format!("{deferred:?}"),
            format!("{:?}", deferred.read().unwrap().value()),
            "`Debug` shows a resident value as the value itself"
        );

        // Readers on other threads see the same value: the field is `Sync` through its owner.
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..1000 {
                        assert_eq!(
                            deferred.read().unwrap().value().unpack_str(),
                            Some("contents")
                        );
                    }
                });
            }
        });
    }

    #[test]
    fn test_deferred_owned_frozen_by_ref() {
        let owned: OwnedFrozen<Value<'static>> =
            OwnedFrozen::build(StarlarkTestHeapName::frozen_heap_name(), |heap| {
                heap.alloc("contents").to_value()
            });
        let deferred = DeferredOwnedFrozen::new(owned);
        let prefix = String::from("read: ");
        let result = deferred
            .by_ref(move |v| {
                let mut result = prefix;
                result.push_str(v.unpack_str().unwrap());
                result
            })
            .unwrap();
        drop(deferred);
        assert_eq!(result, "read: contents");
    }

    #[cfg(feature = "pagable")]
    #[cfg(fbcode_build)]
    mod paging {
        use std::cell::Cell;
        use std::sync::Condvar;
        use std::sync::Mutex;
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;

        use super::*;
        use crate::values::Deferred;
        use crate::values::ThinBoxSliceValue;
        use crate::values::any_complex::StarlarkAnyComplex;

        #[derive(
            Debug,
            ProvidesStaticType,
            Allocative,
            starlark_derive::StarlarkPagable
        )]
        struct DeferredTestFields<'v> {
            value: Deferred<Value<'v>>,
            optional: Deferred<Option<Value<'v>>>,
            children: Deferred<ThinBoxSliceValue<'v>>,
            tail: u32,
        }

        crate::register_starlark_any_complex!(frozen DeferredTestFields<'_>);

        type DeferredTestRoot = DeferredOwnedFrozen<
            ValueTyped<'static, StarlarkAnyComplex<DeferredTestFields<'static>>>,
        >;

        fn deferred_test_root(present: bool, count: usize) -> DeferredTestRoot {
            DeferredOwnedFrozen::new(OwnedFrozen::build(
                StarlarkTestHeapName::frozen_heap_name(),
                |heap| {
                    heap.alloc_simple_typed(StarlarkAnyComplex {
                        value: DeferredTestFields {
                            value: Deferred::new(heap.alloc("single").to_value()),
                            optional: Deferred::new(
                                present.then(|| heap.alloc("optional").to_value()),
                            ),
                            children: Deferred::new(
                                (0..count)
                                    .map(|i| heap.alloc(format!("child-{i}")).to_value())
                                    .collect(),
                            ),
                            tail: 0x12345678,
                        },
                    })
                },
            ))
        }

        #[test]
        fn test_deferred_fields_read_only_on_demand() {
            use pagable::PagableDeserialize;
            use pagable::PagableDeserializer;
            use pagable::PagableSerialize;
            use pagable::testing::TestingDeserializer;
            use pagable::testing::TestingSerializer;

            for present in [false, true] {
                for count in [0, 1, 3] {
                    let source = deferred_test_root(present, count);
                    let mut ser = TestingSerializer::new();
                    source.pagable_serialize(&mut ser).unwrap();
                    let bytes = ser.finish();
                    drop(source);

                    let mut de = TestingDeserializer::new(&bytes);
                    let storage = de.storage();
                    storage
                        .storage_context()
                        .get_or_init(|| crate::pagable::DeferredFieldReadsEnabled);
                    let root = DeferredTestRoot::pagable_deserialize(&mut de).unwrap();
                    drop(de);
                    assert!(
                        root.peek().is_none(),
                        "the owned root itself remains unread"
                    );
                    assert_eq!(format!("{root:?}"), "<unread>");

                    root.by_ref(|root| {
                        let fields = &root.as_ref().value;
                        assert_eq!(
                            fields.tail, 0x12345678,
                            "deferring must consume the exact field wire prefix"
                        );
                        assert!(fields.value.peek().is_none());
                        assert_eq!(fields.children.len(), count);
                        assert_eq!(fields.children.peek().is_none(), count != 0);
                        assert_eq!(fields.optional.peek().is_none(), present);
                        assert_eq!(fields.value.read().unwrap().unpack_str(), Some("single"));
                        assert_eq!(
                            fields.children.peek().is_none(),
                            count != 0,
                            "reading one field does not load its siblings"
                        );
                        assert_eq!(
                            fields
                                .optional
                                .read()
                                .unwrap()
                                .map(|v| v.unpack_str().unwrap()),
                            present.then_some("optional")
                        );
                        assert_eq!(
                            fields
                                .children
                                .read()
                                .unwrap()
                                .iter()
                                .map(|v| v.unpack_str().unwrap().to_owned())
                                .collect::<Vec<_>>(),
                            (0..count).map(|i| format!("child-{i}")).collect::<Vec<_>>(),
                        );
                    })
                    .unwrap();
                    assert!(root.peek().is_some());
                }
            }
        }

        #[test]
        fn test_deferred_owned_concurrent_reads_and_storage_lifetime() {
            use pagable::PagableDeserialize;
            use pagable::PagableDeserializer;
            use pagable::PagableSerialize;
            use pagable::testing::TestingDeserializer;
            use pagable::testing::TestingSerializer;

            let source = deferred_test_root(true, 3);
            let mut ser = TestingSerializer::new();
            source.pagable_serialize(&mut ser).unwrap();
            let bytes = ser.finish();
            drop(source);
            let mut de = TestingDeserializer::new(&bytes);
            let storage = de.storage();
            storage
                .storage_context()
                .get_or_init(|| crate::pagable::DeferredFieldReadsEnabled);
            let root = DeferredTestRoot::pagable_deserialize(&mut de).unwrap();
            drop(de);

            let barrier = std::sync::Barrier::new(8);
            std::thread::scope(|scope| {
                for _ in 0..8 {
                    scope.spawn(|| {
                        barrier.wait();
                        root.by_ref(|root| {
                            let fields = &root.as_ref().value;
                            assert_eq!(fields.children.read().unwrap().len(), 3);
                            assert_eq!(fields.value.read().unwrap().unpack_str(), Some("single"));
                        })
                        .unwrap();
                    });
                }
            });
            let weak = storage.downgrade();
            drop(storage);
            assert!(
                weak.upgrade().is_none(),
                "pending fields must not retain the storage cache"
            );
            root.by_ref(|root| {
                let fields = &root.as_ref().value;
                assert_eq!(
                    fields.value.read().unwrap().unpack_str(),
                    Some("single"),
                    "already read values outlive storage"
                );
                assert!(fields.optional.peek().is_none());
                for _ in 0..2 {
                    let error = fields
                        .optional
                        .read()
                        .expect_err("a read after storage closed must fail");
                    assert!(
                        format!("{error:#}").contains("storage has been closed"),
                        "{error:#}"
                    );
                    assert!(
                        fields.optional.is_pending(),
                        "failed reads leave the field retryable"
                    );
                }
            })
            .unwrap();
        }

        #[derive(Debug, Allocative)]
        struct PanicWhenRead;

        impl crate::pagable::StarlarkSerialize for PanicWhenRead {
            fn starlark_serialize(
                &self,
                _: &mut dyn crate::pagable::StarlarkSerializeContext,
            ) -> crate::Result<()> {
                Ok(())
            }
        }

        impl<'fv> crate::pagable::StarlarkDeserialize<'fv> for PanicWhenRead {
            fn starlark_deserialize(
                _: &mut dyn crate::pagable::StarlarkDeserializeContext<'_, 'fv>,
            ) -> crate::Result<Self> {
                panic!("injected deferred deserializer panic");
            }
        }

        #[derive(
            Debug,
            ProvidesStaticType,
            Allocative,
            starlark_derive::StarlarkPagable
        )]
        struct DeferredPanicData {
            failure: PanicWhenRead,
        }

        crate::register_starlark_any_complex!(frozen DeferredPanicData);

        /// Deserialization entered, and the test's permission to finish it.
        static READ_GATE: (Mutex<(bool, bool)>, Condvar) =
            (Mutex::new((false, false)), Condvar::new());
        static GATED_READS: AtomicUsize = AtomicUsize::new(0);

        /// Parks the resolving thread inside deserialization until the test releases it.
        #[derive(Debug, Allocative)]
        struct BlockWhenRead;

        impl crate::pagable::StarlarkSerialize for BlockWhenRead {
            fn starlark_serialize(
                &self,
                _: &mut dyn crate::pagable::StarlarkSerializeContext,
            ) -> crate::Result<()> {
                Ok(())
            }
        }

        impl<'fv> crate::pagable::StarlarkDeserialize<'fv> for BlockWhenRead {
            fn starlark_deserialize(
                _: &mut dyn crate::pagable::StarlarkDeserializeContext<'_, 'fv>,
            ) -> crate::Result<Self> {
                GATED_READS.fetch_add(1, Ordering::SeqCst);
                let (lock, changed) = &READ_GATE;
                let mut gate = lock.lock().unwrap();
                gate.0 = true;
                changed.notify_all();
                while !gate.1 {
                    gate = changed.wait(gate).unwrap();
                }
                Ok(BlockWhenRead)
            }
        }

        #[derive(
            Debug,
            ProvidesStaticType,
            Allocative,
            starlark_derive::StarlarkPagable
        )]
        struct DeferredBlockData {
            gate: BlockWhenRead,
        }

        crate::register_starlark_any_complex!(frozen DeferredBlockData);

        thread_local! {
            /// The root a `ReenterWhenRead` reads again from inside its own resolution.
            static REENTER: Cell<*const DeferredOwnedFrozen<Value<'static>>> =
                const { Cell::new(std::ptr::null()) };
        }

        /// Reads the field it is being resolved for, as a cyclic value graph would.
        #[derive(Debug, Allocative)]
        struct ReenterWhenRead;

        impl crate::pagable::StarlarkSerialize for ReenterWhenRead {
            fn starlark_serialize(
                &self,
                _: &mut dyn crate::pagable::StarlarkSerializeContext,
            ) -> crate::Result<()> {
                Ok(())
            }
        }

        impl<'fv> crate::pagable::StarlarkDeserialize<'fv> for ReenterWhenRead {
            fn starlark_deserialize(
                _: &mut dyn crate::pagable::StarlarkDeserializeContext<'_, 'fv>,
            ) -> crate::Result<Self> {
                let root = REENTER.get();
                assert!(!root.is_null(), "the test must name the root to re-read");
                // SAFETY: the test keeps the root alive across the read that reaches here.
                let root = unsafe { &*root };
                root.value.read()?;
                Ok(ReenterWhenRead)
            }
        }

        #[derive(
            Debug,
            ProvidesStaticType,
            Allocative,
            starlark_derive::StarlarkPagable
        )]
        struct DeferredReenterData {
            reenter: ReenterWhenRead,
        }

        crate::register_starlark_any_complex!(frozen DeferredReenterData);

        #[test]
        fn test_second_reader_waits_for_the_resolving_thread() {
            use pagable::PagableDeserialize;
            use pagable::PagableDeserializer;
            use pagable::PagableSerialize;
            use pagable::testing::TestingDeserializer;
            use pagable::testing::TestingSerializer;

            use crate::pagable::starlark_deserialize_context::StarlarkDeserWaitGraph;

            *READ_GATE.0.lock().unwrap() = (false, false);
            GATED_READS.store(0, Ordering::SeqCst);
            let source: OwnedFrozen<Value<'static>> =
                OwnedFrozen::build(StarlarkTestHeapName::frozen_heap_name(), |heap| {
                    heap.alloc_simple(StarlarkAnyComplex {
                        value: DeferredBlockData {
                            gate: BlockWhenRead,
                        },
                    })
                });
            let mut ser = TestingSerializer::new();
            source.pagable_serialize(&mut ser).unwrap();
            let bytes = ser.finish();
            drop(source);
            let mut de = TestingDeserializer::new(&bytes);
            let storage = de.storage();
            storage
                .storage_context()
                .get_or_init(|| crate::pagable::DeferredFieldReadsEnabled);
            let root = DeferredOwnedFrozen::<Value<'static>>::pagable_deserialize(&mut de).unwrap();
            drop(de);
            assert!(root.value.is_pending());

            std::thread::scope(|scope| {
                let resolver = scope.spawn(|| {
                    root.read().unwrap();
                });
                {
                    let (lock, changed) = &READ_GATE;
                    let mut gate = lock.lock().unwrap();
                    while !gate.0 {
                        gate = changed.wait(gate).unwrap();
                    }
                }
                assert!(
                    !root.value.is_pending() && root.peek().is_none(),
                    "the resolver holds the field while it deserializes"
                );
                let waiter = scope.spawn(|| {
                    root.read().unwrap();
                });
                let graph = storage
                    .storage_context()
                    .get::<StarlarkDeserWaitGraph>()
                    .expect("page-in created the wait graph");
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                while graph.waiting_threads() == 0 {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the second reader never blocked on the field"
                    );
                    std::thread::yield_now();
                }
                {
                    let (lock, changed) = &READ_GATE;
                    lock.lock().unwrap().1 = true;
                    changed.notify_all();
                }
                resolver.join().unwrap();
                waiter.join().unwrap();
            });
            assert_eq!(
                GATED_READS.load(Ordering::SeqCst),
                1,
                "the waiter takes the resolver's value instead of resolving again"
            );
            assert!(root.peek().is_some());
        }

        #[test]
        fn test_reading_a_field_from_its_own_resolution_is_a_cycle() {
            use pagable::PagableDeserialize;
            use pagable::PagableDeserializer;
            use pagable::PagableSerialize;
            use pagable::testing::TestingDeserializer;
            use pagable::testing::TestingSerializer;

            let source: OwnedFrozen<Value<'static>> =
                OwnedFrozen::build(StarlarkTestHeapName::frozen_heap_name(), |heap| {
                    heap.alloc_simple(StarlarkAnyComplex {
                        value: DeferredReenterData {
                            reenter: ReenterWhenRead,
                        },
                    })
                });
            let mut ser = TestingSerializer::new();
            source.pagable_serialize(&mut ser).unwrap();
            let bytes = ser.finish();
            drop(source);
            let mut de = TestingDeserializer::new(&bytes);
            de.storage()
                .storage_context()
                .get_or_init(|| crate::pagable::DeferredFieldReadsEnabled);
            let root = DeferredOwnedFrozen::<Value<'static>>::pagable_deserialize(&mut de).unwrap();
            REENTER.set(std::ptr::from_ref(&root));
            let error = root
                .value
                .read()
                .expect_err("re-reading a field from its own resolution must fail");
            REENTER.set(std::ptr::null());
            assert!(
                format!("{error:#}").contains("cyclic deferred field read"),
                "{error:#}"
            );
            assert!(
                root.value.is_pending(),
                "the failed read restores the pending record"
            );
        }

        #[test]
        #[cfg(panic = "unwind")]
        fn test_deferred_deserializer_panic_releases_field_and_slot() {
            use pagable::PagableDeserialize;
            use pagable::PagableDeserializer;
            use pagable::PagableSerialize;
            use pagable::testing::TestingDeserializer;
            use pagable::testing::TestingSerializer;

            let source: OwnedFrozen<Value<'static>> =
                OwnedFrozen::build(StarlarkTestHeapName::frozen_heap_name(), |heap| {
                    heap.alloc_simple(StarlarkAnyComplex {
                        value: DeferredPanicData {
                            failure: PanicWhenRead,
                        },
                    })
                });
            let mut ser = TestingSerializer::new();
            source.pagable_serialize(&mut ser).unwrap();
            let bytes = ser.finish();
            drop(source);
            let mut de = TestingDeserializer::new(&bytes);
            de.storage()
                .storage_context()
                .get_or_init(|| crate::pagable::DeferredFieldReadsEnabled);
            let root = DeferredOwnedFrozen::<Value<'static>>::pagable_deserialize(&mut de).unwrap();
            let panic =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| root.read().map(drop)))
                    .expect_err("the injected panic must unwind through the read");
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap();
            assert!(
                message.contains("injected deferred deserializer panic"),
                "{message}"
            );
            assert!(
                root.value.is_pending(),
                "unwinding must restore the pending record"
            );
            let error = root
                .read()
                .err()
                .expect("the recorded slot failure must be reported");
            assert!(
                format!("{error:#}").contains("value deserialization did not complete"),
                "{error:#}"
            );
            assert!(
                root.value.is_pending(),
                "a failed read must restore the pending record"
            );
        }

        #[test]
        fn test_deferred_typed_value_rejects_wrong_type_and_releases_claim() {
            use pagable::PagableDeserialize;
            use pagable::PagableDeserializer;
            use pagable::PagableSerialize;
            use pagable::testing::TestingDeserializer;
            use pagable::testing::TestingSerializer;

            let source: OwnedFrozen<Value<'static>> =
                OwnedFrozen::build(StarlarkTestHeapName::frozen_heap_name(), |heap| {
                    heap.alloc("wrong root type").to_value()
                });
            let mut ser = TestingSerializer::new();
            source.pagable_serialize(&mut ser).unwrap();
            let bytes = ser.finish();
            drop(source);
            let mut de = TestingDeserializer::new(&bytes);
            de.storage()
                .storage_context()
                .get_or_init(|| crate::pagable::DeferredFieldReadsEnabled);
            let root = DeferredTestRoot::pagable_deserialize(&mut de).unwrap();
            for _ in 0..2 {
                let error = root
                    .read()
                    .err()
                    .expect("a deferred typed value must reject a string");
                assert!(format!("{error:#}").contains("resolved to a"), "{error:#}");
                assert!(
                    root.value.is_pending(),
                    "type errors must not leave a resolving sentinel"
                );
            }
        }

        #[test]
        fn test_unread_deferred_owned_reserialization() {
            use pagable::PagableDeserialize;
            use pagable::PagableSerialize;
            use pagable::storage::handle::PagableStorageHandle;
            use pagable::storage::in_memory::InMemoryPagableStorage;
            use pagable::storage::support::SerializerForPaging;
            use pagable::storage::traits::ArcSerCache;

            let backing = InMemoryPagableStorage::new();
            let storage = backing.handle();
            storage
                .storage_context()
                .get_or_init(|| crate::pagable::DeferredFieldReadsEnabled);
            let source = deferred_test_root(true, 3);
            let mut ser = SerializerForPaging::new(storage.storage_context());
            source.pagable_serialize(&mut ser).unwrap();
            let (bytes, arcs) = ser.finish();
            let key = storage
                .page_out_item(bytes, arcs, &ArcSerCache::new(), storage.storage_context())
                .unwrap();
            storage.flush().unwrap();
            drop(source);
            storage.arc_cache().clear();

            let handle = PagableStorageHandle::new(storage.clone());
            let data = storage.fetch_data_blocking(&key).unwrap();
            let mut de = handle.root_deserializer(key, &data);
            let root = DeferredTestRoot::pagable_deserialize(&mut de).unwrap();
            assert!(root.peek().is_none());
            let mut ser = SerializerForPaging::new(storage.storage_context());
            root.pagable_serialize(&mut ser).unwrap();
            let (bytes, arcs) = ser.finish();
            let copied = storage
                .page_out_item(bytes, arcs, &ArcSerCache::new(), storage.storage_context())
                .unwrap();
            assert_eq!(
                key, copied,
                "unread reserialization keeps the wire data and dependency slots unchanged"
            );
            assert!(
                root.peek().is_none(),
                "same-storage serialization must not force a read"
            );

            let other = InMemoryPagableStorage::new();
            let mut ser = SerializerForPaging::new(other.storage_context());
            assert!(
                root.pagable_serialize(&mut ser).is_err(),
                "a stored owner's row cannot be silently reused in unrelated storage"
            );
            drop(ser);

            drop(de);
            drop(root);
            storage.arc_cache().clear();
            let mut de = handle.root_deserializer(copied, &data);
            let restored = DeferredTestRoot::pagable_deserialize(&mut de).unwrap();
            restored
                .by_ref(|root| {
                    assert_eq!(
                        root.as_ref().value.value.read().unwrap().unpack_str(),
                        Some("single")
                    );
                    assert_eq!(root.as_ref().value.children.read().unwrap().len(), 3);
                })
                .unwrap();
        }
    }

    #[cfg(feature = "pagable")]
    mod resident_paging {
        use super::*;
        use crate::values::Deferred;
        use crate::values::ThinBoxSliceValue;
        use crate::values::any_complex::StarlarkAnyComplex;

        #[derive(
            Debug,
            ProvidesStaticType,
            Allocative,
            starlark_derive::StarlarkPagable
        )]
        struct ResidentFields<'v> {
            optional: Deferred<Option<Value<'v>>>,
            children: Deferred<ThinBoxSliceValue<'v>>,
        }

        crate::register_starlark_any_complex!(frozen ResidentFields<'_>);

        #[test]
        fn test_deferred_fields_round_trip_is_resident() {
            use pagable::PagableDeserialize;
            use pagable::PagableSerialize;
            use pagable::testing::TestingDeserializer;
            use pagable::testing::TestingSerializer;

            for present in [false, true] {
                let owned: OwnedFrozen<
                    ValueTyped<'static, StarlarkAnyComplex<ResidentFields<'static>>>,
                > = OwnedFrozen::build(StarlarkTestHeapName::frozen_heap_name(), |heap| {
                    heap.alloc_simple_typed(StarlarkAnyComplex {
                        value: ResidentFields {
                            optional: Deferred::new(present.then(Value::new_none)),
                            children: Deferred::new(ThinBoxSliceValue::from_iter([
                                heap.alloc("a").to_value(),
                                heap.alloc("b").to_value(),
                                heap.alloc("c").to_value(),
                            ])),
                        },
                    })
                });
                let deferred = DeferredOwnedFrozen::new(owned);
                let mut serializer = TestingSerializer::new();
                deferred.pagable_serialize(&mut serializer).unwrap();
                let bytes = serializer.finish();
                drop(deferred);
                let mut deserializer = TestingDeserializer::new(&bytes);
                #[cfg(not(fbcode_build))]
                {
                    use pagable::PagableDeserializer;

                    deserializer
                        .storage_context()
                        .get_or_init(|| crate::pagable::DeferredFieldReadsEnabled);
                }
                let restored = DeferredOwnedFrozen::<ValueTyped<StarlarkAnyComplex<ResidentFields>>>::pagable_deserialize(&mut deserializer).unwrap();
                assert_eq!(
                    restored
                        .by_ref(|root| root.as_ref().value.children.len())
                        .unwrap(),
                    3,
                );
                let fields = &restored
                    .peek()
                    .expect("eager deserialization")
                    .value()
                    .as_ref()
                    .value;
                let optional = fields.optional.peek().expect("resident optional field");
                assert_eq!(optional.is_some(), present);
                if let Some(value) = optional {
                    assert!(value.is_none());
                }
                let children = fields.children.peek().expect("resident slice field");
                assert_eq!(
                    children
                        .iter()
                        .map(|v| v.unpack_str().unwrap())
                        .collect::<Vec<_>>(),
                    ["a", "b", "c"]
                );
            }
        }
    }
}
