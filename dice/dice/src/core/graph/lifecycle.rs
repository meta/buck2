/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use dupe::Dupe;
use pagable::DataKey;

use crate::storage::EvictionPolicy;
use crate::storage::PageOutPolicy;
use crate::value::DiceValidValue;
use crate::value::MaybeResident;

/// A retained value together with where it sits in the page-out lifecycle. Every state except
/// `PagedOut` keeps the value resident; `PagedOut` keeps only the on-disk `DataKey`.
#[derive(Allocative, Debug)]
pub(crate) enum PagableValue {
    /// Resident, no on-disk copy: a candidate for the next page-out, which evicts it.
    NeverPagedOut(DiceValidValue),
    /// Serialized to disk and evicted from memory.
    PagedOut(DataKey),
    /// Resident but not serializable (e.g. `NoValueSerialize`, or a serialization error); never
    /// a page-out candidate.
    NonPageable(DiceValidValue),
    /// Resident again after a page-out, with a value no row holds. Under
    /// [`PageOutPolicy::OncePerValue`] a candidate for the next page-out, which writes it and
    /// leaves it resident; under [`PageOutPolicy::OncePerKey`] never paged out again.
    Recomputed(DiceValidValue),
    /// Resident, with a row holding an equal value: read back from it, or written while it
    /// stayed resident, under [`PageOutPolicy::OncePerValue`]. Evicted without a write when
    /// memory is short.
    Backed(Box<BackedValue>),
}

/// What [`PagableValue::row_committed`] did with a value.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RowCommitted {
    Evicted,
    Backed,
    Unchanged,
}

#[derive(Allocative, Debug)]
pub(crate) struct BackedValue {
    pub(crate) value: DiceValidValue,
    pub(crate) data_key: DataKey,
    /// Size of the row's top-level serialized blob, excluding nested `PagableArc` sub-values;
    /// zero when the value became backed without its row being read or written.
    pub(crate) serialized_bytes: u64,
    /// When the value became backed; see [`residency_cutoff`].
    residency_generation: u64,
}

static RESIDENCY_GENERATION: AtomicU64 = AtomicU64::new(0);

/// A generation no earlier than that of any value backed so far, and earlier than that of any
/// value backed from now on.
pub(crate) fn residency_cutoff() -> u64 {
    RESIDENCY_GENERATION.load(Ordering::Relaxed)
}

impl BackedValue {
    fn new(value: DiceValidValue, data_key: DataKey, serialized_bytes: u64) -> Box<Self> {
        Box::new(Self {
            value,
            data_key,
            serialized_bytes,
            residency_generation: RESIDENCY_GENERATION.fetch_add(1, Ordering::Relaxed) + 1,
        })
    }
}

mini_vec::size_assert::words_of_type!(PagableValue, 3);

impl PagableValue {
    /// The resident value, if any.
    pub(crate) fn as_hydrated(&self) -> Option<&DiceValidValue> {
        match self {
            PagableValue::NeverPagedOut(value)
            | PagableValue::NonPageable(value)
            | PagableValue::Recomputed(value) => Some(value),
            PagableValue::Backed(backed) => Some(&backed.value),
            PagableValue::PagedOut(_) => None,
        }
    }

    /// The on-disk key of a paged-out value; `None` for resident values.
    pub(crate) fn data_key(&self) -> Option<DataKey> {
        match self {
            PagableValue::PagedOut(key) => Some(*key),
            _ => None,
        }
    }

    /// Whether the next page-out serializes this value.
    pub(crate) fn is_page_out_candidate(&self, policy: PageOutPolicy) -> bool {
        match self {
            PagableValue::NeverPagedOut(_) => true,
            PagableValue::Recomputed(_) => policy == PageOutPolicy::OncePerValue,
            PagableValue::PagedOut(_) | PagableValue::NonPageable(_) | PagableValue::Backed(_) => {
                false
            }
        }
    }

    /// The value as a lookup hands it out.
    pub(crate) fn as_maybe_resident(&self) -> MaybeResident<DiceValidValue> {
        match self {
            PagableValue::PagedOut(key) => MaybeResident::PagedOut(*key),
            resident => MaybeResident::Resident(
                resident
                    .as_hydrated()
                    .expect("every state but PagedOut is resident")
                    .dupe(),
            ),
        }
    }

    /// The state a value newly retained for a key takes, given the state of the key's most
    /// recently retained value: a key whose value has been paged out once stays resident at
    /// page-out from then on.
    pub(crate) fn lifecycle_after(previous: Option<&PagableValue>) -> fn(DiceValidValue) -> Self {
        match previous {
            None | Some(PagableValue::NeverPagedOut(_)) => PagableValue::NeverPagedOut,
            Some(PagableValue::NonPageable(_)) => PagableValue::NonPageable,
            Some(
                PagableValue::Recomputed(_) | PagableValue::PagedOut(_) | PagableValue::Backed(_),
            ) => PagableValue::Recomputed,
        }
    }

    /// Makes a paged-out value resident again with `value`, which was read back from its
    /// `DataKey` (a row of `serialized_bytes`) or has just been proven equal to it (pass zero).
    /// Under [`PageOutPolicy::OncePerValue`] the row keeps backing it.
    pub(crate) fn make_resident(
        &mut self,
        value: DiceValidValue,
        serialized_bytes: u64,
        policy: PageOutPolicy,
    ) {
        if let PagableValue::PagedOut(data_key) = *self {
            *self = match policy {
                PageOutPolicy::OncePerKey => PagableValue::Recomputed(value),
                PageOutPolicy::OncePerValue => {
                    PagableValue::Backed(BackedValue::new(value, data_key, serialized_bytes))
                }
            };
        }
    }

    /// The row at `data_key`, of `serialized_bytes`, now holds this value. A value never paged
    /// out is evicted under [`EvictionPolicy::OnIdle`]; otherwise it, like a recomputed one,
    /// stays resident, backed by the row.
    pub(crate) fn row_committed(
        &mut self,
        data_key: DataKey,
        serialized_bytes: u64,
        policy: EvictionPolicy,
    ) -> RowCommitted {
        let backed =
            |value| PagableValue::Backed(BackedValue::new(value, data_key, serialized_bytes));
        match std::mem::replace(self, PagableValue::PagedOut(data_key)) {
            PagableValue::NeverPagedOut(value) => match policy {
                EvictionPolicy::OnIdle => RowCommitted::Evicted,
                EvictionPolicy::OnMemoryPressure => {
                    *self = backed(value);
                    RowCommitted::Backed
                }
            },
            PagableValue::Recomputed(value) => {
                *self = backed(value);
                RowCommitted::Backed
            }
            other => {
                *self = other;
                RowCommitted::Unchanged
            }
        }
    }

    /// Drops a value backed no later than `cutoff` (see [`residency_cutoff`]), leaving its row.
    /// Returns whether there was one.
    pub(crate) fn evict_backed(&mut self, cutoff: u64) -> bool {
        let PagableValue::Backed(backed) = self else {
            return false;
        };
        if backed.residency_generation > cutoff {
            return false;
        }
        let data_key = backed.data_key;
        *self = PagableValue::PagedOut(data_key);
        true
    }
}
