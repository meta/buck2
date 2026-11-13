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

//! Explicit access to fields that can be materialized on demand.
//!
//! [`Deferred::new`] stores an already available value. [`Deferred::read`]
//! returns the value, materializing it if necessary; [`Deferred::peek`] only
//! inspects whether it is available. Cloning materializes the value, but
//! formatting with `Debug` and memory accounting do not. A deferred slice's
//! length is known without materializing its elements, although reading that
//! metadata can wait for another thread.
//!
//! Constructors store resident values. Deserialization can defer heap-backed
//! fields when enabled for the storage; it is eager by default. Unread fields
//! require that storage to remain open, and report a read failure if it closes.

use std::fmt;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::ptr::NonNull;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering;

use allocative::Allocative;

use crate::pagable::StarlarkDeserialize;
use crate::pagable::StarlarkDeserializeContext;
use crate::pagable::StarlarkSerialize;
use crate::pagable::StarlarkSerializeContext;
use crate::values::Freeze;
use crate::values::FreezeError;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::StarlarkValue;
use crate::values::ThinBoxSliceValue;
use crate::values::Trace;
use crate::values::Tracer;
use crate::values::Value;
use crate::values::ValueTyped;
use crate::values::layout::pointer::TAG_DEFERRED;
use crate::values::layout::pointer::TAG_MASK;

#[cfg(fbcode_build)]
mod pending;
#[cfg(not(fbcode_build))]
mod pending {
    //! The published pagable dependency lacks weak storage handles. Keep OSS
    //! deserialization eager, without creating a storage/cache ownership cycle.

    use std::convert::Infallible;
    use std::marker::PhantomData;

    use super::Deferred;
    use super::DeferredWord;
    use super::deferred_error;
    use crate::pagable::StarlarkDeserializeContext;
    use crate::pagable::StarlarkSerializeContext;
    use crate::pagable::serialized_frozen_value::SerializedFrozenValue;
    use crate::values::Value;

    /// Uninhabited until the published pagable dependency supports deferred reads.
    #[doc(hidden)]
    pub struct DeferredReadContext<'fv>(Infallible, PhantomData<Value<'fv>>);

    impl<'fv> DeferredReadContext<'fv> {
        /// # Safety
        /// No instance of this context can be constructed.
        pub(super) unsafe fn deserialize<T: DeferredWord>(
            self,
            _ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
        ) -> crate::Result<Deferred<T>> {
            match self.0 {}
        }
    }

    /// Aligned as the real `Pending` is, so the tag-bit assertion holds on every target.
    #[derive(Clone)]
    #[repr(align(8))]
    pub(super) struct Pending {
        pub(super) refs: Box<[SerializedFrozenValue]>,
        unavailable: Infallible,
    }

    impl Pending {
        pub(super) fn try_serialize(
            &self,
            _ctx: &mut dyn StarlarkSerializeContext,
        ) -> crate::Result<bool> {
            match self.unavailable {}
        }
    }

    pub(super) fn resolve<T: DeferredWord>(_field: &Deferred<T>) -> crate::Result<()> {
        Err(deferred_error(
            "deferred field reads are unavailable in this build",
        ))
    }

    pub(super) fn with_pending<T: DeferredWord, R>(
        _field: &Deferred<T>,
        _f: impl FnOnce(&Pending) -> R,
    ) -> Option<R> {
        None
    }
    /// Nothing is ever unread here.
    pub(super) fn prefetch_many<'a, T: DeferredWord + 'a>(
        _fields: impl IntoIterator<Item = &'a Deferred<T>>,
    ) -> crate::Result<()> {
        Ok(())
    }
}

pub use pending::DeferredReadContext;
use pending::Pending;

/// Pointer layout selected by the sealed deferred-word implementations.
/// Reading a field records the exact wire prefix it found under this shape.
#[derive(Clone, Copy)]
pub enum DeferredShape {
    /// A single untyped or type-checked value pointer.
    Value,
    /// A presence bit followed by at most one pointer.
    Optional,
    /// The thin slice's singleton/allocated discriminator and value pointers.
    Slice,
}

mod private {
    use super::*;

    pub trait Sealed: Sized {
        const SHAPE: DeferredShape;

        /// # Safety
        /// The destination brand must retain every heap of the frozen input
        /// values, or be private erased storage paired with that exact owner.
        unsafe fn from_frozen_values<'fv>(
            values: &mut dyn Iterator<Item = crate::Result<Value<'fv>>>,
        ) -> crate::Result<Self>;
    }
}

/// The one-word types a [`Deferred`] can hold.
///
/// # Safety
///
/// `Self` is one pointer: it has the size and alignment of one, its bits are a
/// pointer's (so that storing it as one keeps its provenance), and it never
/// carries `TAG_DEFERRED` in its low bits. So a `&Self` may be formed over a
/// word holding a valid `Self`, and the tag alone tells a value from an unread
/// field.
pub unsafe trait DeferredWord: Sized + private::Sealed {
    /// The value pointer type the field's wire form is made of.
    type Element;

    /// Builds the value from its resolved pointers, in wire order.
    fn from_elements(
        elements: &mut dyn Iterator<Item = crate::Result<Self::Element>>,
    ) -> crate::Result<Self>;
}

impl<'v> private::Sealed for Value<'v> {
    const SHAPE: DeferredShape = DeferredShape::Value;

    unsafe fn from_frozen_values<'fv>(
        values: &mut dyn Iterator<Item = crate::Result<Value<'fv>>>,
    ) -> crate::Result<Self> {
        unsafe { from_rebranded_values(values) }
    }
}
// SAFETY: `Value` is a `repr(transparent)` non-null tagged pointer whose tags are the
// `PointerTags`; `TAG_DEFERRED` is in `TAGS_NEVER_VALUE`.
unsafe impl<'v> DeferredWord for Value<'v> {
    type Element = Value<'v>;

    fn from_elements(
        elements: &mut dyn Iterator<Item = crate::Result<Self::Element>>,
    ) -> crate::Result<Self> {
        let value = elements
            .next()
            .ok_or_else(|| deferred_error("a deferred value has no pointer"))??;
        if elements.next().is_some() {
            return Err(deferred_error("a deferred value has more than one pointer"));
        }
        Ok(value)
    }
}

impl<'v> private::Sealed for Option<Value<'v>> {
    const SHAPE: DeferredShape = DeferredShape::Optional;

    unsafe fn from_frozen_values<'fv>(
        values: &mut dyn Iterator<Item = crate::Result<Value<'fv>>>,
    ) -> crate::Result<Self> {
        unsafe { from_rebranded_values(values) }
    }
}
// SAFETY: `Value` is transparent over a non-null word, so its `Option` uses zero for
// `None` and otherwise has the same representation and reserved tags as `Value`.
unsafe impl<'v> DeferredWord for Option<Value<'v>> {
    type Element = Value<'v>;

    fn from_elements(
        elements: &mut dyn Iterator<Item = crate::Result<Self::Element>>,
    ) -> crate::Result<Self> {
        let value = elements.next().transpose()?;
        if elements.next().is_some() {
            return Err(deferred_error(
                "a deferred optional value has more than one pointer",
            ));
        }
        Ok(value)
    }
}

impl<'v, T: StarlarkValue<'v>> private::Sealed for ValueTyped<'v, T> {
    const SHAPE: DeferredShape = DeferredShape::Value;

    unsafe fn from_frozen_values<'fv>(
        values: &mut dyn Iterator<Item = crate::Result<Value<'fv>>>,
    ) -> crate::Result<Self> {
        unsafe { from_rebranded_values(values) }
    }
}
// SAFETY: `repr(transparent)` over a `Value`.
unsafe impl<'v, T: StarlarkValue<'v>> DeferredWord for ValueTyped<'v, T> {
    type Element = Value<'v>;

    fn from_elements(
        elements: &mut dyn Iterator<Item = crate::Result<Self::Element>>,
    ) -> crate::Result<Self> {
        let value = Value::from_elements(elements)?;
        ValueTyped::new(value).ok_or_else(|| {
            deferred_error(&format!(
                "a deferred `{}` resolved to a `{}`",
                T::TYPE,
                value.get_type()
            ))
        })
    }
}

impl<'v> private::Sealed for ThinBoxSliceValue<'v> {
    const SHAPE: DeferredShape = DeferredShape::Slice;

    unsafe fn from_frozen_values<'fv>(
        values: &mut dyn Iterator<Item = crate::Result<Value<'fv>>>,
    ) -> crate::Result<Self> {
        unsafe { from_rebranded_values(values) }
    }
}
// SAFETY: `repr(transparent)` over a word that is either a `Value` or a slice allocation tagged
// with one of the two patterns `thin_box` owns; `TAG_DEFERRED` is the third.
unsafe impl<'v> DeferredWord for ThinBoxSliceValue<'v> {
    type Element = Value<'v>;

    fn from_elements(
        elements: &mut dyn Iterator<Item = crate::Result<Self::Element>>,
    ) -> crate::Result<Self> {
        elements.collect()
    }
}

/// Rebrands resolved frozen pointers to the destination brand and builds the value.
///
/// # Safety
/// As [`private::Sealed::from_frozen_values`].
unsafe fn from_rebranded_values<'v, 'fv, T: DeferredWord<Element = Value<'v>>>(
    values: &mut dyn Iterator<Item = crate::Result<Value<'fv>>>,
) -> crate::Result<T> {
    // SAFETY: the caller's precondition: the destination brand retains every
    // input value's heap.
    let mut values =
        values.map(|value| value.map(|value| unsafe { value.rebrand_frozen_unchecked() }));
    T::from_elements(&mut values)
}

const _: () = assert!(mem::size_of::<Value<'static>>() == mem::size_of::<usize>());
const _: () = assert!(mem::size_of::<ThinBoxSliceValue<'static>>() == mem::size_of::<usize>());
const _: () = assert!(mem::align_of::<Pending>() > TAG_MASK);

fn deferred_error(message: &str) -> crate::Error {
    crate::Error::new_kind(crate::ErrorKind::DeferredRead(anyhow::anyhow!("{message}")))
}

/// The address of the word of a field whose pointers one thread is resolving right now.
const RESOLVING: usize = TAG_DEFERRED;

/// A one-word field whose value can be materialized on demand.
///
/// [`new`](Self::new) stores an already available value. [`read`](Self::read)
/// returns the value, materializing it if necessary, while [`peek`](Self::peek)
/// never materializes it. Reads may block; a storage failure is an error that
/// leaves the field unread, so the read can be retried.
/// `Clone` reads the value; `Debug` and memory accounting only inspect it.
/// Deserialization is eager by default; storage may opt into deferred reads.
/// OSS builds read eagerly until their pagable dependency supports weak storage handles.
///
/// Reading preserves `T`'s lifetime: its pointers refer only to heaps retained
/// by the owning heap. The wrapper itself does not own a heap; use
/// [`DeferredOwnedFrozen`](crate::values::DeferredOwnedFrozen) to retain one.
///
/// One word inside a heap value, holding a pointer into a retained heap, so it
/// cannot be a `PagableArc`: a heap is evicted as a whole once nothing refers
/// to it, never field by field.
pub struct Deferred<T: DeferredWord> {
    /// A valid `T`; or `TAG_DEFERRED | Box<Pending>` (pending); or
    /// `TAG_DEFERRED` alone (resolving). Accessed atomically until resolved,
    /// plainly after. Null is a resident `None` for an optional value.
    ///
    /// Atomic so that shared readers and any future writer meet through the
    /// atomic memory model; a word holding a value is never written again, so a
    /// reference into it taken after an acquire load reads settled bits.
    word: AtomicPtr<()>,
    _marker: PhantomData<T>,
}

// SAFETY: the word is one of a `T` (whose auto traits are required), a box only the resolving
// thread touches, or a tag; every transition is an atomic on the word.
unsafe impl<T: DeferredWord + Send> Send for Deferred<T> {}
unsafe impl<T: DeferredWord + Sync> Sync for Deferred<T> {}

enum State {
    Resolved,
    Pending(NonNull<Pending>),
    Resolving,
}

impl<T: DeferredWord> Deferred<T> {
    /// Resolves the unread fields among `fields` concurrently, so that reading
    /// each afterwards finds it resident. For code that is about to read many
    /// fields in turn, which would otherwise resolve them one at a time.
    pub fn prefetch_many<'a>(fields: impl IntoIterator<Item = &'a Deferred<T>>) -> crate::Result<()>
    where
        T: 'a,
    {
        pending::prefetch_many(fields)
    }

    /// Constructs a field with an already available value.
    pub fn new(value: T) -> Self {
        let word = ptr::from_ref(&value).cast::<*mut ()>();
        // SAFETY: `DeferredWord` promises a one-pointer representation.
        let word = unsafe { word.read() };
        mem::forget(value);
        debug_assert!(
            word.addr() & TAG_MASK != TAG_DEFERRED,
            "a `DeferredWord` is never tagged deferred"
        );
        Self::from_word(word)
    }

    /// # Safety
    /// The pending origin must own `T`'s heap brand, or `T` must be private
    /// erased storage paired with that owner and exposed only through branded access.
    #[cfg(fbcode_build)]
    unsafe fn pending(pending: Box<Pending>) -> Self {
        let pending = Box::into_raw(pending).cast::<()>();
        debug_assert!(pending.addr() & TAG_MASK == 0);
        Self::from_word(pending.map_addr(|addr| addr | TAG_DEFERRED))
    }

    fn from_word(word: *mut ()) -> Self {
        Self {
            word: AtomicPtr::new(word),
            _marker: PhantomData,
        }
    }

    fn state_of(word: *mut ()) -> State {
        let addr = word.addr();
        if addr & TAG_MASK != TAG_DEFERRED {
            State::Resolved
        } else if addr == RESOLVING {
            State::Resolving
        } else {
            let pending = word.map_addr(|addr| addr & !TAG_MASK).cast::<Pending>();
            // SAFETY: a pending word is a box's address under the tag.
            State::Pending(unsafe { NonNull::new_unchecked(pending) })
        }
    }

    fn state(&self) -> State {
        Self::state_of(self.word.load(Ordering::Acquire))
    }

    /// Returns the value if available, or `None` if pending or being resolved.
    /// Never materializes the value or waits for resolution.
    pub fn peek(&self) -> Option<&T> {
        match self.state() {
            // SAFETY: a resolved word holds a valid `T`. Its write happens before the
            // Acquire load in `state` that saw it, and it is never written again while
            // shared, so the returned reference cannot race a write.
            State::Resolved => Some(unsafe { &*self.word.as_ptr().cast::<T>() }),
            State::Pending(_) | State::Resolving => None,
        }
    }

    /// Whether the field is unread and not being resolved: a failed read must leave it so.
    #[cfg(all(test, fbcode_build))]
    pub(crate) fn is_pending(&self) -> bool {
        matches!(self.state(), State::Pending(_))
    }

    fn peek_mut(&mut self) -> Option<&mut T> {
        match Self::state_of(*self.word.get_mut()) {
            // SAFETY: as in `peek`, and `&mut self` excludes every other access.
            State::Resolved => {
                Some(unsafe { &mut *ptr::from_mut(self.word.get_mut()).cast::<T>() })
            }
            State::Pending(_) | State::Resolving => None,
        }
    }

    /// Returns the value, materializing it if necessary.
    /// May wait for another thread to finish resolving it.
    ///
    /// Fails if storage is closed, the row is gone or malformed, or a read
    /// cycle would require returning a value before it is initialized. The
    /// field stays unread after a failure.
    pub fn read(&self) -> crate::Result<&T> {
        if let Some(value) = self.peek() {
            return Ok(value);
        }
        self.resolve_slow()
    }

    #[cold]
    fn resolve_slow(&self) -> crate::Result<&T> {
        pending::resolve(self)?;
        Ok(self.peek().expect("resolution published a value"))
    }

    /// # Safety
    /// The result must be kept private inside an owning carrier for the exact
    /// heap represented by `ctx`, exposing `T` only through branded borrows.
    pub(crate) unsafe fn deserialize_owned<'fv>(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        if let Some(context) = ctx.deferred_read_context() {
            // SAFETY: the owning carrier preserves this context's heap/brand.
            unsafe { context.deserialize::<T>(ctx) }
        } else {
            let value = ctx.deserialize_value()?;
            // SAFETY: the result is private erased storage paired with the
            // context's exact heap owner, as required by this method.
            Ok(Self::new(unsafe {
                T::from_frozen_values(&mut std::iter::once(Ok(value)))?
            }))
        }
    }

    /// The value, for a field that has been read; `None` for one that has not.
    /// Consumes the field either way.
    fn into_resolved(mut self) -> Option<T> {
        let word = *self.word.get_mut();
        match Self::state_of(word) {
            State::Resolved => {
                mem::forget(self);
                // SAFETY: a resolved word is the bits of a valid `T` (`new` and `resolve_slow`
                // write nothing else), and `self` is forgotten, so this is the one owner.
                Some(unsafe { ptr::from_ref(&word).cast::<T>().read() })
            }
            State::Pending(_) | State::Resolving => None,
        }
    }

    fn with_pending<R>(&self, f: impl FnOnce(&Pending) -> R) -> Option<R> {
        pending::with_pending(self, f)
    }
}

impl<'v> Deferred<ThinBoxSliceValue<'v>> {
    /// The number of values, without materializing them.
    /// May wait while another thread holds the pending metadata.
    pub fn len(&self) -> usize {
        loop {
            if let Some(values) = self.peek() {
                return values.len();
            }
            if let Some(len) = self.with_pending(|pending| pending.refs.len()) {
                return len;
            }
            std::thread::yield_now();
        }
    }

    /// Whether there are no values, without materializing them.
    /// May wait as [`len`](Self::len) does.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Cloning an unread field copies its saved pointers, so the clone is unread too.
impl<T: DeferredWord + Clone> Clone for Deferred<T> {
    fn clone(&self) -> Self {
        loop {
            if let Some(value) = self.peek() {
                return Deferred::new(value.clone());
            }
            // Builds without `fbcode_build` read every field eagerly, so none is pending.
            #[cfg(fbcode_build)]
            if let Some(pending) = self.with_pending(Clone::clone) {
                // SAFETY: the clone holds the same `T` for the same owner as the original.
                return unsafe { Deferred::pending(Box::new(pending)) };
            }
            std::thread::yield_now();
        }
    }
}

impl<T: DeferredWord> Drop for Deferred<T> {
    fn drop(&mut self) {
        match Self::state_of(*self.word.get_mut()) {
            // SAFETY: a valid `T` this field owns, and `&mut self`
            // excludes other access.
            State::Resolved => unsafe {
                ptr::drop_in_place(ptr::from_mut(self.word.get_mut()).cast::<T>())
            },
            // SAFETY: an unread field still owns its record.
            State::Pending(pending) => drop(unsafe { Box::from_raw(pending.as_ptr()) }),
            // A resolver holds `&self`, so the field cannot be dropped while resolving.
            State::Resolving => unreachable!("a deferred field dropped while being resolved"),
        }
    }
}

impl<T: DeferredWord + fmt::Debug> fmt::Debug for Deferred<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Deliberately never reads: a `{:?}` reaching an unread field is the accident this
        // type exists to avoid.
        match self.peek() {
            Some(value) => fmt::Debug::fmt(value, f),
            None => f.write_str("<unread>"),
        }
    }
}

impl<T: DeferredWord + Allocative> Allocative for Deferred<T> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        match self.peek() {
            Some(value) => visitor.visit_field(allocative::Key::new("value"), value),
            None => {
                let pending = self
                    .with_pending(|pending| {
                        mem::size_of::<Pending>() + mem::size_of_val::<[_]>(&pending.refs)
                    })
                    .unwrap_or(0);
                visitor.visit_simple(allocative::Key::new("pending"), pending);
            }
        }
        visitor.exit();
    }
}

// SAFETY: only frozen heaps are paged, so every traceable field is resident.
unsafe impl<'v, T: DeferredWord + Trace<'v>> Trace<'v> for Deferred<T> {
    fn trace(&mut self, tracer: &Tracer<'v>) {
        if let Some(value) = self.peek_mut() {
            value.trace(tracer);
        }
    }
}

impl<'v, T> Freeze<'v> for Deferred<T>
where
    T: DeferredWord + Freeze<'v>,
    for<'fv> T::Frozen<'fv>: DeferredWord,
{
    type Frozen<'fv> = Deferred<T::Frozen<'fv>>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        let value = self.into_resolved().ok_or_else(|| {
            FreezeError::new("an unread deferred field cannot be frozen".to_owned())
        })?;
        Ok(Deferred::new(value.freeze(freezer)?))
    }
}

impl<T: DeferredWord + StarlarkSerialize> StarlarkSerialize for Deferred<T> {
    fn starlark_serialize(&self, ctx: &mut dyn StarlarkSerializeContext) -> crate::Result<()> {
        if let Some(value) = self.peek() {
            return value.starlark_serialize(ctx);
        }
        // The snapshot does not hold the registry lock across serializer callbacks.
        if let Some(pending) = self.with_pending(Clone::clone) {
            if pending.try_serialize(ctx)? {
                return Ok(());
            }
        }
        self.read()?.starlark_serialize(ctx)
    }
}

impl<'fv, T> StarlarkDeserialize<'fv> for Deferred<T>
where
    T: DeferredWord<Element = Value<'fv>> + StarlarkDeserialize<'fv>,
{
    fn starlark_deserialize(
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Self> {
        match ctx.deferred_read_context() {
            // SAFETY: the associated element equality ties T to this context's
            // exact brand; the context captures the corresponding origin.
            Some(context) => unsafe { context.deserialize::<T>(ctx) },
            None => Ok(Deferred::new(T::starlark_deserialize(ctx)?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::mem;

    use super::*;
    use crate::values::FrozenHeap;
    use crate::values::Heap;

    #[test]
    fn test_size() {
        assert_eq!(mem::size_of::<Deferred<Value>>(), mem::size_of::<usize>());
        assert_eq!(
            mem::size_of::<Deferred<ThinBoxSliceValue>>(),
            mem::size_of::<usize>()
        );
        assert_eq!(
            mem::size_of::<Deferred<Option<Value>>>(),
            mem::size_of::<usize>()
        );
        assert_eq!(
            mem::size_of::<Option<Deferred<Value>>>(),
            2 * mem::size_of::<usize>()
        );
    }

    #[test]
    fn test_resident_value() {
        Heap::temp(|heap| {
            let value = heap.alloc("resident").to_value();
            let deferred = Deferred::new(value);
            assert!(deferred.peek().unwrap().ptr_eq(value));
            assert!(deferred.read().unwrap().ptr_eq(value));
            assert_eq!(format!("{deferred:?}"), format!("{value:?}"));
        });
    }

    #[test]
    fn test_optional_value_preserves_starlark_none() {
        let absent = Deferred::new(None::<Value>);
        let present = Deferred::new(Some(Value::new_none()));
        assert!(absent.read().unwrap().is_none());
        assert!(
            present
                .read()
                .unwrap()
                .expect("present Starlark None")
                .is_none()
        );
        assert!(absent.clone().read().unwrap().is_none());
        assert!(present.clone().read().unwrap().is_some());
    }

    #[test]
    fn test_resident_slice() {
        Heap::temp(|heap| {
            let values = [
                heap.alloc("a").to_value(),
                Value::testing_new_int(1),
                heap.alloc_list(&[]),
            ];
            let deferred = Deferred::new(ThinBoxSliceValue::from_iter(values));
            assert_eq!(deferred.len(), 3);
            for (got, want) in deferred.read().unwrap().iter().zip(values) {
                assert!(got.ptr_eq(want));
            }
        });
    }

    #[test]
    fn test_resident_typed_is_one_word() {
        FrozenHeap::temp(|heap| {
            let s = heap.alloc("typed");
            let typed: ValueTyped<crate::values::string::str_type::StarlarkStr> =
                ValueTyped::new(s.to_value()).unwrap();
            let deferred = Deferred::new(typed);
            assert_eq!(mem::size_of_val(&deferred), mem::size_of::<usize>());
            assert!(deferred.read().unwrap().to_value().ptr_eq(s.to_value()));
        });
    }

    #[test]
    fn test_resident_drops_its_value() {
        // A resident slice owns its allocation; dropping the field frees it exactly once
        // (under ASAN a double free or a leak fails the test).
        Heap::temp(|heap| {
            let values: Vec<_> = (0..4)
                .map(|i| heap.alloc(i.to_string()).to_value())
                .collect();
            let deferred = Deferred::new(ThinBoxSliceValue::from_iter(values));
            assert_eq!(deferred.len(), 4);
            drop(deferred);
        });
    }
}
