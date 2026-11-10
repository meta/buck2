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

//! The unfrozen heap ([`Heap`]) and its garbage collector ([`Tracer`]).

use std::cell::Cell;
use std::cell::RefCell;
use std::cell::RefMut;
use std::cmp;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::marker::PhantomData;
use std::mem;
use std::mem::MaybeUninit;
use std::ptr::NonNull;

use bumpalo::Bump;
use dupe::Dupe;

use crate::collections::StarlarkHashValue;
use crate::eval::runtime::profile::instant::ProfilerInstant;
use crate::values::AllocValue;
use crate::values::HeapSendable;
#[cfg(test)]
use crate::values::OwnedFrozen;
use crate::values::OwnedFrozenRef;
use crate::values::StringValue;
use crate::values::Trace;
use crate::values::ValueTyped;
use crate::values::layout::avalue::AValue;
use crate::values::layout::avalue::AValueImpl;
use crate::values::layout::heap::arena::Arena;
use crate::values::layout::heap::arena::ArenaVisitor;
use crate::values::layout::heap::arena::HeapKind;
use crate::values::layout::heap::arena::ValueReservation;
use crate::values::layout::heap::branding::rebrand_ref_unchecked;
use crate::values::layout::heap::call_enter_exit::CallEnter;
use crate::values::layout::heap::call_enter_exit::CallExit;
use crate::values::layout::heap::call_enter_exit::NeedsDrop;
use crate::values::layout::heap::call_enter_exit::NoDrop;
use crate::values::layout::heap::fast_cell::FastCell;
use crate::values::layout::heap::frozen::HeapReferences;
use crate::values::layout::heap::profile::by_type::HeapSummary;
use crate::values::layout::heap::repr::AValueHeapEntryState;
use crate::values::layout::value::Value;
use crate::values::string::intern::interner::StringValueInterner;

/// An owned heap on which [`Value`]s can be allocated.
///
/// Private for now, but there's no reason it couldn't be public as long as access is restricted to
/// branded functions with signatures like those of `Heap::temp`
struct OwnedHeap {
    /// Peak memory seen when a garbage collection takes place (may be lower than currently allocated)
    peak_allocated: Cell<usize>,
    arena: FastCell<Arena<Bump>>,
    /// The strings interned on this heap. Like the arena, this is the heap's own storage, held
    /// with the brand erased because the heap has none until a [`Heap`] handle names it;
    /// `string_interner` restores it.
    str_interner: RefCell<StringValueInterner<'static>>,
    /// Memory I depend on.
    refs: HeapReferences,
    ban_gc: Cell<bool>,
}

impl OwnedHeap {
    /// Create a new [`OwnedHeap`].
    fn new() -> Self {
        Self {
            peak_allocated: Default::default(),
            arena: Default::default(),
            str_interner: Default::default(),
            refs: Default::default(),
            ban_gc: Cell::new(true),
        }
    }
}

/// An unfrozen heap: the values allocated on it may be mutable and are garbage collected. Each
/// [`Module`](crate::environment::Module) has one.
///
/// The handle is `Copy` and exists only inside a closure ([`Heap::temp`],
/// [`Module::with_temp_heap`](crate::environment::Module::with_temp_heap)); the values allocated
/// on it are branded with the closure's lifetime `'v`, see the `branding` module.
#[derive(Copy, Clone, Dupe)]
// `PhantomData` is needed to make the type invariant in `'v` - without that, branding doesn't mean
// anything.
pub struct Heap<'v>(&'v OwnedHeap, PhantomData<fn(&'v ()) -> &'v ()>);

impl<'v> Debug for Heap<'v> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut x = f.debug_struct("Heap");
        x.field(
            "bytes",
            &self.0.arena.try_borrow().map(|x| x.allocated_bytes()),
        );
        x.finish()
    }
}

impl<'v> Heap<'v> {
    /// Create a heap and use it within the closure
    ///
    /// Heap is discarded at the end of the closure.
    pub fn temp<F, R>(f: F) -> R
    where
        F: for<'v2> FnOnce(Heap<'v2>) -> R,
    {
        let heap = OwnedHeap::new();
        f(Heap(&heap, PhantomData))
    }

    /// Like `temp`, but `async`
    pub async fn temp_async<F, R>(f: F) -> R
    where
        F: for<'v2> AsyncFnOnce(Heap<'v2>) -> R,
    {
        // It's interesting to note that this is in fact more expressive than `temp` alone. While
        // it's possible for `temp` to return a future which the user can then await externally,
        // that future can't capture a reference to the heap. Here though, we allow the future
        // "returned" by this function to do so. We make that sound by also capturing the heap
        // itself in the future.
        let heap = OwnedHeap::new();
        f(Heap(&heap, PhantomData)).await
    }

    pub(in crate::values::layout) fn string_interner(self) -> RefMut<'v, StringValueInterner<'v>> {
        // SAFETY: Every entry was allocated in this heap, which `'v` names.
        let interner: &'v RefCell<StringValueInterner<'v>> =
            unsafe { rebrand_ref_unchecked(&self.0.str_interner) };
        interner.borrow_mut()
    }

    pub(crate) fn trace_interner(self, tracer: &Tracer<'v>) {
        self.string_interner().trace(tracer);
    }

    #[cfg(test)]
    pub(crate) fn referenced_heaps(self) -> Vec<OwnedFrozen<()>> {
        self.0.refs.to_vec()
    }

    /// The set of heaps this heap depends on, for `ModuleHeaps` to share with the frozen heap it
    /// builds alongside this one.
    pub(in crate::values::layout::heap) fn references(self) -> HeapReferences {
        self.0.refs.dupe()
    }

    /// Add a dependency onto the provided frozen heap.
    pub fn add_reference(self, h: OwnedFrozenRef<'_, ()>) {
        self.0.refs.insert(h);
    }
}

impl<'v> Heap<'v> {
    /// Number of bytes allocated on this heap, not including any memory
    /// allocated outside of the starlark heap.
    pub fn allocated_bytes(self) -> usize {
        self.0.arena.borrow().allocated_bytes()
    }

    /// Peak memory allocated to this heap, even if the value is now lower
    /// as a result of a subsequent garbage collection.
    pub fn peak_allocated_bytes(self) -> usize {
        cmp::max(self.allocated_bytes(), self.0.peak_allocated.get())
    }

    /// Number of bytes allocated by the heap but not yet filled.
    pub fn available_bytes(self) -> usize {
        self.0.arena.borrow().available_bytes()
    }

    pub(in crate::values::layout) fn alloc_raw<A>(
        self,
        x: AValueImpl<'v, A>,
    ) -> ValueTyped<'v, A::StarlarkValue>
    where
        A: AValue<'v, ExtraElem = ()>,
        A::StarlarkValue: HeapSendable<'v>,
    {
        let arena = self.0.arena.borrow();
        let v = arena.alloc(x);
        // SAFETY: just allocated in this heap, by the allocation's pointer.
        unsafe { ValueTyped::new_repr(v) }
    }

    pub(in crate::values::layout) fn alloc_raw_extra<A>(
        self,
        x: AValueImpl<'v, A>,
    ) -> (
        ValueTyped<'v, A::StarlarkValue>,
        *mut [MaybeUninit<A::ExtraElem>],
    )
    where
        A: AValue<'v>,
        A::StarlarkValue: HeapSendable<'v>,
    {
        let arena = self.0.arena.borrow();
        let (v, extra) = arena.alloc_extra(x);
        // SAFETY: just allocated in this heap, by the allocation's pointer.
        let v = unsafe { ValueTyped::new_repr(NonNull::new_unchecked(v)) };
        (v, extra)
    }

    pub(in crate::values::layout) fn alloc_str_init(
        self,
        len: usize,
        hash: StarlarkHashValue,
        init: impl FnOnce(*mut u8),
    ) -> StringValue<'v> {
        let arena = self.0.arena.borrow();
        let v = arena.alloc_str_init(len, hash, init);

        // We have an arena inside a RefCell which stores ValueMem<'v>
        // However, we promise not to clear the RefCell other than for GC
        // so we can make the `arena` available longer
        unsafe {
            let value = Value::new_ptr(NonNull::new_unchecked(v), true);
            StringValue::new_unchecked(value)
        }
    }

    /// Allocate a new value on a [`Heap`].
    pub fn alloc<T: AllocValue<'v>>(self, x: T) -> Value<'v> {
        x.alloc_value(self).to_value()
    }

    pub(crate) unsafe fn visit_arena(
        self,
        forward_heap_kind: HeapKind,
        v: &mut impl ArenaVisitor<'v>,
    ) {
        unsafe { (*self.0.arena.get_mut()).visit_arena(HeapKind::Unfrozen, forward_heap_kind, v) }
    }

    /// Allow gcing in this heap
    ///
    /// # SAFETY
    ///
    /// This is basically impossible to reason about, hence its existence in the first place
    pub(crate) unsafe fn allow_gc(self) {
        self.0.ban_gc.set(false);
    }

    /// Garbage collect any values that are unused. This function is _unsafe_ in
    /// the sense that any `Value<'v>` not returned by `Tracer` _will become
    /// invalid_. Furthermore, any references to values, e.g `&'v str` will
    /// also become invalid.
    pub(crate) unsafe fn garbage_collect(self, f: impl FnOnce(&Tracer<'v>)) {
        if self.0.ban_gc.get() {
            return;
        }

        unsafe {
            // Record the highest peak, so it never decreases
            self.0.peak_allocated.set(self.peak_allocated_bytes());
            self.garbage_collect_internal(f)
        }
    }

    unsafe fn garbage_collect_internal(self, f: impl FnOnce(&Tracer<'v>)) {
        unsafe {
            // Must rewrite all Value's so they point at the new heap.
            // Take the arena out of the heap to make sure nobody allocates in it,
            // but hold the reference until the GC is done.
            let _arena = self.0.arena.take();

            let tracer = Tracer::<'v> {
                arena: Arena::default(),
                phantom: PhantomData,
            };
            f(&tracer);
            self.0.arena.set(tracer.arena);
        }
    }

    /// Obtain a summary of how much memory is currently allocated by this heap.
    pub fn allocated_summary(self) -> HeapSummary {
        self.0.arena.borrow().allocated_summary()
    }

    pub(crate) fn record_call_enter(self, function: Value<'v>) {
        let time = ProfilerInstant::now();
        assert!(mem::needs_drop::<CallEnter<NeedsDrop>>());
        assert!(!mem::needs_drop::<CallEnter<NoDrop>>());
        self.alloc_complex_no_freeze(CallEnter {
            function,
            time,
            maybe_drop: NeedsDrop,
        });
        self.alloc_complex_no_freeze(CallEnter {
            function,
            time,
            maybe_drop: NoDrop,
        });
    }

    pub(crate) fn record_call_exit(self) {
        let time = ProfilerInstant::now();
        assert!(mem::needs_drop::<CallExit<NeedsDrop>>());
        assert!(!mem::needs_drop::<CallExit<NoDrop>>());
        self.alloc_simple(CallExit {
            time,
            maybe_drop: NeedsDrop,
        });
        self.alloc_simple(CallExit {
            time,
            maybe_drop: NoDrop,
        });
    }
}

/// Used to perform garbage collection by [`Trace::trace`](crate::values::Trace::trace).
pub struct Tracer<'v> {
    arena: Arena<Bump>,
    // Invariant in the brand, like every carrier of one.
    phantom: PhantomData<fn(&'v ()) -> &'v ()>,
}

impl<'v> Tracer<'v> {
    /// Walk over a value during garbage collection.
    pub fn trace(&self, value: &mut Value<'v>) {
        *value = self.adjust(*value)
    }

    /// Helper function to annotate that this field has been considered for tracing,
    /// but is not relevant because it has a static lifetime containing no relevant values.
    /// Does nothing.
    pub fn trace_static<T: ?Sized + 'static>(&self, value: &T) {
        // Nothing to do because T can't contain the lifetime 'v
        let _ = value;
    }

    pub(crate) fn reserve<T: AValue<'v, ExtraElem = ()>>(&self) -> ValueReservation<'v, T> {
        let (r, extra) = self.reserve_with_extra::<T>(0);
        let extra = unsafe { &mut *extra };
        debug_assert!(extra.is_empty());
        r
    }

    pub(crate) fn reserve_with_extra<T: AValue<'v>>(
        &self,
        extra_len: usize,
    ) -> (ValueReservation<'v, T>, *mut [MaybeUninit<T::ExtraElem>]) {
        assert!(!T::IS_STR, "strings cannot be reserved");
        let (r, extra) = self.arena.reserve_with_extra::<T>(extra_len);
        (ValueReservation(r), extra)
    }

    pub(crate) fn alloc_str(&self, x: &str) -> Value<'v> {
        let v = self.arena.alloc_str(x);
        // SAFETY: just allocated in this heap, by the allocation's pointer.
        unsafe { Value::new_ptr(NonNull::new_unchecked(v), true) }
    }

    fn adjust(&self, value: Value<'v>) -> Value<'v> {
        // Case 1, doesn't point at the old arena
        if !value.0.is_unfrozen() {
            return value;
        }
        let old_val = value.0.unpack_ptr().unwrap();

        // Case 2: We have already been replaced with a forwarding, or need to freeze
        match old_val.state() {
            AValueHeapEntryState::Forward(x) => unsafe { x.forward_ptr().unpack_unfrozen_value() },
            // SAFETY: just checked to be a live value.
            AValueHeapEntryState::Value(_) => unsafe {
                value
                    .0
                    .value_header_ptr_unchecked()
                    .unpack()
                    .heap_copy(self)
            },
            AValueHeapEntryState::Reservation(_) => {
                unreachable!("a heap reservation cannot appear in the source heap")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use starlark_derive::starlark_module;

    use super::Heap;
    use crate as starlark;
    use crate::assert::Assert;
    use crate::environment::GlobalsBuilder;
    use crate::values::StringValue;

    #[test]
    fn test_string_reallocated_on_heap() {
        Heap::temp(|heap| {
            let first = heap.alloc_str("xx");
            let second = heap.alloc_str("xx");
            assert!(
                !first.to_value().ptr_eq(second.to_value()),
                "Plain allocations should recreate values. Note assertion negation."
            );
        });
    }

    #[test]
    fn test_interned_string_equal() {
        Heap::temp(|heap| {
            let first = heap.alloc_str_intern("xx");
            let second = heap.alloc_str_intern("xx");
            assert!(
                first.to_value().ptr_eq(second.to_value()),
                "Interned allocations should be equal."
            );
        });
    }

    #[starlark_module]
    fn validate_str_interning(globals: &mut GlobalsBuilder) {
        fn append_x<'v>(str: StringValue<'v>, heap: Heap<'v>) -> anyhow::Result<StringValue<'v>> {
            Ok(heap.alloc_str_intern(&(str.as_str().to_owned() + "x")))
        }
    }

    #[test]
    fn test_interned_str_starlark() {
        let mut a = Assert::new();
        a.globals_add(validate_str_interning);

        a.pass(
            r#"
x = append_x("foo")
assert_eq(x, "foox")
garbage_collect()
assert_eq(x, "foox")
        "#,
        );
    }
}
