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

//! Resolution of an unread [`Deferred`] field.
//!
//! Three structures coordinate the readers of one field:
//! - The process-wide registry behind [`active_reads`], keyed by field
//!   address. A field under resolution holds only `RESOLVING`, so the registry
//!   is what keeps its [`Pending`] record reachable for metadata readers and
//!   what hands the record to exactly one resolver. It is global because the
//!   field is a single word with no room for a ticket.
//! - The storage's `StarlarkDeserWaitGraph`, in which the resolver claims
//!   `DeserWaitKey::Field(address)` and a blocked reader records its wait, so
//!   a read that would wait on a value under construction on the same thread,
//!   or the reverse, is reported as a cycle rather than deadlocking.
//! - A per-read [`ActiveRead`] condvar that later readers of the same field
//!   block on until the resolver publishes.
//!
//! Lock order: the registry, then a ticket's `done` mutex, then the wait
//! graph's locks. The registry is never held across a wait or a load.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;
use std::sync::atomic::Ordering;

use pagable::PagableDeserialize;
use pagable::PagableSerialize;
use pagable::storage::handle::PagableStorageHandle;
use pagable::storage::handle::WeakPagableStorageHandle;

use super::Deferred;
use super::DeferredShape;
use super::DeferredWord;
use super::RESOLVING;
use super::State;
use super::deferred_error;
use crate::pagable::StarlarkDeserializeContext;
use crate::pagable::StarlarkSerializeContext;
use crate::pagable::serialized_frozen_value::SerializedFrozenValue;
use crate::pagable::starlark_deserialize_context::ClaimGuard;
use crate::pagable::starlark_deserialize_context::DeferredResolveContext;
use crate::pagable::starlark_deserialize_context::DeserWaitKey;
use crate::pagable::starlark_deserialize_context::StarlarkDeserScope;
use crate::pagable::starlark_deserialize_context::StarlarkDeserWaitGraph;
use crate::pagable::starlark_deserialize_context::with_deferred_values;
use crate::values::Value;
use crate::values::layout::heap::sealed::WeakFrozenHeapRef;
use crate::values::layout::pointer::TAG_DEFERRED;

/// The discriminator that preceded a field's pointers on the wire, kept so an
/// unread field reserializes byte for byte. [`DeferredShape`] is the type-level
/// layout that says how to read a field; a slice shape covers two prefixes.
#[derive(Clone, Copy)]
enum WirePrefix {
    Value,
    Optional,
    SliceSingleton,
    SliceAllocated,
}

/// An opaque deserialization capability tied to the owning heap's brand.
#[doc(hidden)]
pub struct DeferredReadContext<'fv> {
    scope: Arc<StarlarkDeserScope>,
    storage: PagableStorageHandle,
    origin: WeakFrozenHeapRef,
    brand: PhantomData<Value<'fv>>,
}

impl<'fv> DeferredReadContext<'fv> {
    /// # Safety
    /// `origin` must be the heap whose brand is `'fv`, with the supplied
    /// scope and storage belonging to the same deserialization.
    pub(crate) unsafe fn new(
        scope: Arc<StarlarkDeserScope>,
        storage: PagableStorageHandle,
        origin: WeakFrozenHeapRef,
    ) -> Self {
        Self {
            scope,
            storage,
            origin,
            brand: PhantomData,
        }
    }

    /// # Safety
    /// `T` must have this context's brand, or be erased storage paired with
    /// this exact heap owner and accessed only through branded borrows.
    pub(super) unsafe fn deserialize<T: DeferredWord>(
        self,
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> crate::Result<Deferred<T>> {
        let (prefix, count) = match T::SHAPE {
            DeferredShape::Value => (WirePrefix::Value, 1),
            DeferredShape::Optional => (
                WirePrefix::Optional,
                usize::from(bool::pagable_deserialize(ctx.pagable())?),
            ),
            DeferredShape::Slice => match u8::pagable_deserialize(ctx.pagable())? {
                0 => (WirePrefix::SliceSingleton, 1),
                1 => (
                    WirePrefix::SliceAllocated,
                    usize::pagable_deserialize(ctx.pagable())?,
                ),
                tag => {
                    return Err(deferred_error(&format!(
                        "invalid deferred slice tag: {tag}"
                    )));
                }
            },
        };
        // Do not reserve from an untrusted count before reading any elements.
        let mut refs = Vec::new();
        for _ in 0..count {
            refs.push(SerializedFrozenValue::pagable_deserialize(ctx.pagable())?);
        }
        let has_heap_ref = refs
            .iter()
            .any(|value| matches!(value, SerializedFrozenValue::HeapPtr { .. }));
        let pending = Pending {
            refs: refs.into_boxed_slice(),
            prefix,
            scope: self.scope,
            storage: self.storage.downgrade(),
            origin: self.origin,
        };
        if has_heap_ref {
            // SAFETY: the adapter's precondition ties `T` to the captured origin.
            Ok(unsafe { Deferred::pending(Box::new(pending)) })
        } else {
            // SAFETY: the same brand/owner condition holds, and these references
            // are all immortal values (or the field is empty).
            Ok(Deferred::new(unsafe { pending.resolve::<T>()? }))
        }
    }
}

/// The saved pointers of an unread field and what resolving them needs.
///
/// `scope` is held strongly: resolution needs it and it does not own the
/// field. `storage` is weak because the storage's cache owns the heap that
/// owns this field, and a strong handle would keep that cache alive from
/// inside itself. `origin` is weak because the owning heap owns
/// this field. Both are upgraded on each read; a closed storage or a dropped
/// owner is a read error.
/// Aligned so the low tag bits of a `Box<Pending>` address are free on every target,
/// including 32-bit ones where pointer alignment alone would not clear them.
#[derive(Clone)]
#[repr(align(8))]
pub(super) struct Pending {
    pub(super) refs: Box<[SerializedFrozenValue]>,
    prefix: WirePrefix,
    scope: Arc<StarlarkDeserScope>,
    storage: WeakPagableStorageHandle,
    origin: WeakFrozenHeapRef,
}

impl Pending {
    /// # Safety
    /// The destination `T` has the captured origin's brand, or is private
    /// erased storage whose owner preserves that brand on every access.
    unsafe fn resolve<T: DeferredWord>(&self) -> crate::Result<T> {
        let storage = self
            .storage
            .upgrade()
            .ok_or_else(|| deferred_error("deferred field storage has been closed"))?;
        let origin = self
            .origin
            .upgrade()
            .ok_or_else(|| deferred_error("deferred field owner has been dropped"))?;
        with_deferred_values(
            &self.refs,
            DeferredResolveContext {
                scope: &self.scope,
                storage: &storage,
                origin: &origin,
            },
            |values| {
                // SAFETY: resolution retains each target on `origin`, and the
                // adapter's precondition identifies it with the destination brand.
                unsafe { T::from_frozen_values(values) }
            },
        )
    }

    pub(super) fn try_serialize(
        &self,
        ctx: &mut dyn StarlarkSerializeContext,
    ) -> crate::Result<bool> {
        let storage = self
            .storage
            .upgrade()
            .ok_or_else(|| deferred_error("deferred field storage has been closed"))?;
        if !ptr::eq(storage.storage_context(), ctx.pagable().storage_context()) {
            return Err(deferred_error(
                "serializing a deferred field into different storage is unsupported",
            ));
        }
        if ctx.serialization_scope().root != Some(self.origin.heap_ptr()) {
            return Ok(false);
        }
        // The serialized owner retains its original row and dependency closure.
        // Copy only pointer bytes: adding an arc here would corrupt the cursor.
        match self.prefix {
            WirePrefix::Value => {}
            WirePrefix::Optional => (!self.refs.is_empty()).pagable_serialize(ctx.pagable())?,
            WirePrefix::SliceSingleton => 0u8.pagable_serialize(ctx.pagable())?,
            WirePrefix::SliceAllocated => {
                1u8.pagable_serialize(ctx.pagable())?;
                self.refs.len().pagable_serialize(ctx.pagable())?;
            }
        }
        for value in &self.refs {
            value.pagable_serialize(ctx.pagable())?;
        }
        Ok(true)
    }
}

struct ActiveRead {
    /// The field's address, which is its key in the registry.
    field: usize,
    pending: NonNull<Pending>,
    graph: Arc<StarlarkDeserWaitGraph>,
    done: Mutex<bool>,
    changed: Condvar,
}

// SAFETY: `pending` is a record the field owns; it is read only under the registry lock, which
// also serializes its being freed, and the other fields are `Send` and `Sync` on their own.
unsafe impl Send for ActiveRead {}
unsafe impl Sync for ActiveRead {}

fn active_reads() -> MutexGuard<'static, HashMap<usize, Arc<ActiveRead>>> {
    static READS: OnceLock<Mutex<HashMap<usize, Arc<ActiveRead>>>> = OnceLock::new();
    // Metadata callbacks never mutate the registry; unwinding through one
    // must not prevent a resolving guard from restoring its field.
    READS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

impl ActiveRead {
    fn wait(&self) -> crate::Result<()> {
        let mut done = self
            .done
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if *done {
            return Ok(());
        }
        let (_wait, cycle) = self.graph.begin_wait_and_check_cycle(
            std::thread::current().id(),
            DeserWaitKey::Field(self.field),
        );
        if cycle {
            return Err(deferred_error("cyclic deferred field read"));
        }
        while !*done {
            done = self
                .changed
                .wait(done)
                .unwrap_or_else(|poison| poison.into_inner());
        }
        Ok(())
    }

    fn finish(&self) {
        *self
            .done
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = true;
        self.changed.notify_all();
    }
}

struct PendingClaim<'a, T: DeferredWord> {
    field: &'a Deferred<T>,
    pending: Option<Box<Pending>>,
    active: Arc<ActiveRead>,
    graph_claim: Option<ClaimGuard>,
}

impl<T: DeferredWord> PendingClaim<'_, T> {
    fn finish(&mut self, word: *mut ()) {
        let mut reads = active_reads();
        self.graph_claim.take();
        self.field.word.store(word, Ordering::Release);
        reads.remove(&self.active.field);
        self.active.finish();
        drop(reads);
    }

    fn resolve(mut self) -> crate::Result<()> {
        // SAFETY: only the branded deserialize adapter can construct pending
        // fields, and this claim still borrows that field and its live owner.
        let value = unsafe {
            self.pending
                .as_ref()
                .expect("claim owns its pending record")
                .resolve::<T>()?
        };
        // SAFETY: `DeferredWord` guarantees a one-pointer representation.
        let word = unsafe { ptr::from_ref(&value).cast::<*mut ()>().read() };
        mem::forget(value);
        let pending = self.pending.take();
        self.finish(word);
        drop(pending);
        Ok(())
    }
}

impl<T: DeferredWord> Drop for PendingClaim<'_, T> {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.take() {
            let pending = Box::into_raw(pending).cast::<()>();
            self.finish(pending.map_addr(|addr| addr | TAG_DEFERRED));
        }
    }
}

/// Resolve or wait without leaving the field claimed on any error or unwind.
pub(super) fn resolve<T: DeferredWord>(field: &Deferred<T>) -> crate::Result<()> {
    loop {
        let mut reads = active_reads();
        match field.state() {
            State::Resolved => return Ok(()),
            State::Resolving => {
                let active = reads
                    .get(&(ptr::from_ref(field) as usize))
                    .expect("resolving fields have an active ticket")
                    .clone();
                drop(reads);
                active.wait()?;
            }
            State::Pending(pending) => {
                // SAFETY: every claim/publication holds `reads`, so the box
                // cannot be removed while inspecting it here.
                let storage = unsafe { pending.as_ref() }
                    .storage
                    .upgrade()
                    .ok_or_else(|| deferred_error("deferred field storage has been closed"))?;
                let graph = storage
                    .storage_context()
                    .get_or_init(StarlarkDeserWaitGraph::default);
                let address = ptr::from_ref(field) as usize;
                let active = Arc::new(ActiveRead {
                    field: address,
                    pending,
                    graph,
                    done: Mutex::new(false),
                    changed: Condvar::new(),
                });
                let graph_claim = active
                    .graph
                    .claim(DeserWaitKey::Field(address), std::thread::current().id());
                reads.insert(address, active.clone());
                field
                    .word
                    .store(ptr::without_provenance_mut(RESOLVING), Ordering::Release);
                // SAFETY: the registry lock and state transition transfer the
                // field's unique box to this claim. Metadata readers use the lock.
                let claim = PendingClaim {
                    field,
                    pending: Some(unsafe { Box::from_raw(pending.as_ptr()) }),
                    active,
                    graph_claim: Some(graph_claim),
                };
                drop(reads);
                return claim.resolve();
            }
        }
    }
}

/// Copy metadata while excluding publication/freeing. `f` must not re-enter
/// deferred access or perform I/O; serialization works on a cloned snapshot.
/// Resolves the unread fields among `fields` with the resolutions overlapping
/// on the current runtime's blocking pool, so that reading them afterwards
/// finds them resident. Fields already read cost nothing. Returns only once
/// every resolution has finished, whether or not one failed.
pub(super) fn prefetch_many<'a, T: DeferredWord + 'a>(
    fields: impl IntoIterator<Item = &'a Deferred<T>>,
) -> crate::Result<()> {
    let mut storage: Option<PagableStorageHandle> = None;
    let mut fields_to_load: Vec<FieldPtr> = Vec::new();
    for field in fields {
        let unread = with_pending(field, |pending| {
            pending
                .storage
                .upgrade()
                .ok_or_else(|| deferred_error("deferred field storage has been closed"))
        });
        let Some(handle) = unread else { continue };
        storage.get_or_insert(handle?);
        fields_to_load.push(FieldPtr(NonNull::from(field).cast()));
    }
    let Some(storage) = storage else {
        return Ok(());
    };
    fields_to_load.sort_unstable_by_key(|field| field.0.addr());
    fields_to_load.dedup_by_key(|field| field.0.addr());
    // A function pointer names no lifetime, so the loads can be `'static`
    // although `T` is not.
    let resolve_at: unsafe fn(FieldPtr) -> crate::Result<()> = resolve_at::<T>;
    let loads = fields_to_load
        .into_iter()
        .map(|field| {
            Box::new(move || {
                // SAFETY: `field` came from a borrow of the field that the
                // caller of `prefetch_many` holds until `load_many` returns,
                // and `load_many` returns only after this load has.
                unsafe { resolve_at(field) }.map_err(crate::Error::into_anyhow)
            }) as Box<dyn FnOnce() -> pagable::Result<()> + Send>
        })
        .collect();
    storage.load_many(loads).map_err(crate::Error::new_other)
}

/// A `Deferred<T>` with `T` erased, for a load to carry to the blocking pool.
#[derive(Copy, Clone)]
struct FieldPtr(NonNull<()>);

// SAFETY: Only `resolve_at` uses the pointer, under the contract `prefetch_many` states.
unsafe impl Send for FieldPtr {}

/// Resolves the field `field` points at.
///
/// # Safety
/// `field` must point to a live `Deferred<T>` for the duration of the call.
unsafe fn resolve_at<T: DeferredWord>(field: FieldPtr) -> crate::Result<()> {
    // SAFETY: the caller's contract.
    resolve(unsafe { field.0.cast::<Deferred<T>>().as_ref() })
}

pub(super) fn with_pending<T: DeferredWord, R>(
    field: &Deferred<T>,
    f: impl FnOnce(&Pending) -> R,
) -> Option<R> {
    let reads = active_reads();
    let pending = match field.state() {
        State::Resolved => return None,
        State::Pending(pending) => pending,
        State::Resolving => {
            reads
                .get(&(ptr::from_ref(field) as usize))
                .expect("resolving fields have an active ticket")
                .pending
        }
    };
    // SAFETY: the field borrow excludes Drop, and the registry lock excludes
    // a resolver publishing and freeing this immutable pending record.
    Some(f(unsafe { pending.as_ref() }))
}
