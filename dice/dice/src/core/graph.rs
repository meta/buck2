/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The value layer of the core state (`docs/incrementality.md` Appendix A) over `dice_core`:
//! the values behind revisions, their interning, retention and page-out lifecycle.

use allocative::Allocative;
use bit_set::BitSet;
use dice_core::BranchId;
use dice_core::Cert;
use dice_core::Change;
use dice_core::CoreState;
use dice_core::Env;
use dice_core::KeyMap;
use dice_core::Lookup;
use dupe::Dupe;
use pagable::DataKey;
use smallvec::SmallVec;

use crate::api::invalidation_tracking::InvalidationSources;
use crate::api::key::InvalidationSourcePriority;
use crate::api::storage_type::StorageType;
use crate::arc::Arc;
use crate::core::graph::lifecycle::PagableValue;
use crate::core::graph::lifecycle::RowCommitted;
use crate::core::graph::revision::EpsilonToken;
use crate::core::graph::revision::Revision;
use crate::core::graph::revision::RevisionMint;
use crate::core::graph::types::Candidate;
use crate::core::graph::types::VersionedGraphKey;
use crate::core::graph::types::VersionedGraphResult;
use crate::deps::graph::SeriesParallelDeps;
use crate::dice::PagableNodeCounts;
use crate::key::DiceKey;
use crate::storage::EvictionPolicy;
use crate::storage::PageOutPolicy;
use crate::updater::ChangeType;
use crate::value::DiceValidValue;
use crate::value::MaybeResident;
use crate::value::MaybeResidentComputedValue;
use crate::value::PageOutResult;
use crate::value::ResidentComputedValue;
use crate::versions::VersionNumber;

pub(crate) mod introspection;
pub(crate) mod lifecycle;
pub(crate) mod revision;
#[cfg(test)]
mod tests;
pub(crate) mod types;

/// Dice's environment for the core state: the deps of a certificate keep their series-parallel
/// shape, every assertion carries its invalidation source priority, and the attribute of a
/// derivation is its invalidation sources.
#[derive(Allocative)]
pub(crate) struct DiceEnv;

impl Env for DiceEnv {
    type Premises = SeriesParallelDeps;
    type AssertionData = InvalidationSourcePriority;
    type Attribute = InvalidationSources;
    const EMPTY: InvalidationSources = InvalidationSources::NONE;

    fn assertion(at: VersionNumber, priority: InvalidationSourcePriority) -> InvalidationSources {
        InvalidationSources::source(priority, at)
    }

    fn combine(a: InvalidationSources, b: InvalidationSources) -> InvalidationSources {
        a.merge(b)
    }
}

pub(crate) type DiceCert = Cert<SeriesParallelDeps>;

mini_vec::size_assert::words_of_type!(DiceCert, 4);
mini_vec::size_assert::words_of_type!(dice_core::Claim<DiceEnv>, 4);
mini_vec::size_assert::words_of_type!(dice_core::Slot<DiceEnv>, 6);
mini_vec::size_assert::words_of_type!(KeyValues, 6);
mini_vec::size_assert::words_of_type!(Candidate, 5);

/// A value produced by running the key's computation: Appendix A's fused write.
pub(crate) struct ComputedValueUpdate {
    pub(crate) value: DiceValidValue,
    pub(crate) deps: SeriesParallelDeps,
    pub(crate) epsilon: EpsilonToken,
}

/// The values retained for one key.
#[derive(Allocative, Default)]
struct KeyValues {
    mint: RevisionMint,
    /// Whether the key is an `InjectedKey`, which takes no part in paging. A computed key whose
    /// values are asserted (buck2 does this for its pageable starlark roots) pages like any other.
    injected: bool,
    entries: SmallVec<[(Revision, PagableValue); 1]>,
}

impl KeyValues {
    fn entry(&self, revision: Revision) -> Option<&PagableValue> {
        self.entries
            .iter()
            .find_map(|(r, v)| (*r == revision).then_some(v))
    }

    fn entry_mut(&mut self, revision: Revision) -> Option<&mut PagableValue> {
        self.entries
            .iter_mut()
            .find_map(|(r, v)| (*r == revision).then_some(v))
    }

    fn values(&self) -> impl Iterator<Item = &PagableValue> {
        self.entries.iter().map(|(_, value)| value)
    }

    fn values_mut(&mut self) -> impl Iterator<Item = &mut PagableValue> {
        self.entries.iter_mut().map(|(_, value)| value)
    }

    fn lifecycle(&self) -> fn(DiceValidValue) -> PagableValue {
        PagableValue::lifecycle_after(self.entries.last().map(|(_, v)| v))
    }

    fn insert(&mut self, revision: Revision, value: PagableValue) {
        debug_assert!(self.entry(revision).is_none());
        self.entries.push((revision, value));
    }

    /// Appendix A step 1: the value behind `revision` is the canonical one for the trace, and
    /// `value` is discarded, unless the stored value is paged out and `value` takes its place.
    fn adopt(
        &mut self,
        revision: Revision,
        value: DiceValidValue,
        policy: PageOutPolicy,
    ) -> DiceValidValue {
        match self.entry_mut(revision) {
            Some(stored) => {
                stored.make_resident(value, 0, policy);
                stored.as_hydrated().expect("made resident above").dupe()
            }
            None => {
                let lifecycle = self.lifecycle();
                self.insert(revision, lifecycle(value.dupe()));
                value
            }
        }
    }

    /// Appendix A steps 2 and 3. Returns the revision and the canonical value behind it.
    fn intern_computed(&mut self, value: DiceValidValue) -> (Revision, DiceValidValue) {
        for (revision, stored) in &self.entries {
            if let Some(existing) = stored.as_hydrated()
                && existing.equality(&value)
            {
                return (*revision, existing.dupe());
            }
        }
        let revision = self.mint.mint();
        let lifecycle = self.lifecycle();
        self.insert(revision, lifecycle(value.dupe()));
        (revision, value)
    }

    /// An injected value, compared against every retained value so that a key returned to an
    /// earlier value gets that value's revision back.
    fn intern_asserted(&mut self, value: DiceValidValue, storage: StorageType) -> Revision {
        self.injected = matches!(storage, StorageType::Injected);
        for (revision, stored) in &self.entries {
            if stored.as_hydrated().is_some_and(|e| e.equality(&value)) {
                return *revision;
            }
        }
        let revision = self.mint.mint();
        let entry = if self.injected {
            PagableValue::NonPageable(value)
        } else {
            (self.lifecycle())(value)
        };
        self.insert(revision, entry);
        revision
    }

    /// The value behind a re-issued certificate: the retained one if there is one, made resident
    /// from `value` if it can be, else `value` itself, retained.
    fn restore(
        &mut self,
        revision: Revision,
        value: MaybeResident<DiceValidValue>,
        policy: PageOutPolicy,
    ) -> MaybeResident<DiceValidValue> {
        match self.entry_mut(revision) {
            Some(stored) => {
                if let MaybeResident::Resident(value) = value {
                    stored.make_resident(value, 0, policy);
                }
                stored.as_maybe_resident()
            }
            None => {
                let entry = match value {
                    MaybeResident::Resident(value) => (self.lifecycle())(value),
                    MaybeResident::PagedOut(data_key) => PagableValue::PagedOut(data_key),
                };
                let out = entry.as_maybe_resident();
                self.insert(revision, entry);
                out
            }
        }
    }

    fn contribution(&self, policy: PageOutPolicy) -> Contribution {
        let mut c = Contribution::default();
        if self.injected {
            return c;
        }
        for value in self.values() {
            match value {
                PagableValue::PagedOut(_) => c.paged_out += 1,
                PagableValue::NeverPagedOut(_) | PagableValue::Recomputed(_) => {
                    c.resident += 1;
                    if value.is_page_out_candidate(policy) {
                        c.candidates += 1;
                    }
                }
                PagableValue::Backed(backed) => {
                    c.resident += 1;
                    c.backed += 1;
                    c.backed_bytes += backed.serialized_bytes;
                }
                PagableValue::NonPageable(_) => c.resident += 1,
            }
        }
        c
    }
}

/// A key's share of the paging tallies.
#[derive(Default, Copy, Clone, PartialEq, Eq)]
struct Contribution {
    resident: usize,
    paged_out: usize,
    candidates: usize,
    backed: usize,
    backed_bytes: u64,
}

/// The page-out candidate and backed-value indexes and the resident / paged-out tallies, kept
/// in step with the values by [`VersionedGraph::with_values`] so that no operation needs a scan.
#[derive(Default)]
struct PagableIndex {
    /// Keys with at least one page-out candidate, by `DiceKey::index`.
    candidates: BitSet,
    candidate_count: usize,
    /// Keys with at least one backed value, by `DiceKey::index`.
    backed: BitSet,
    backed_count: usize,
    backed_bytes: u64,
    resident_count: usize,
    paged_out_count: usize,
}

impl PagableIndex {
    fn apply(&mut self, key: DiceKey, before: Contribution, after: Contribution) {
        if before == after {
            return;
        }
        self.resident_count = self.resident_count + after.resident - before.resident;
        self.paged_out_count = self.paged_out_count + after.paged_out - before.paged_out;
        self.candidate_count = self.candidate_count + after.candidates - before.candidates;
        self.backed_count = self.backed_count + after.backed - before.backed;
        self.backed_bytes = self.backed_bytes + after.backed_bytes - before.backed_bytes;
        Self::track(
            &mut self.candidates,
            key,
            before.candidates,
            after.candidates,
        );
        Self::track(&mut self.backed, key, before.backed, after.backed);
    }

    fn track(set: &mut BitSet, key: DiceKey, before: usize, after: usize) {
        match (before > 0, after > 0) {
            (false, true) => {
                set.insert(key.index as usize);
            }
            (true, false) => {
                set.remove(key.index as usize);
            }
            _ => {}
        }
    }
}

/// The keys with a backed value at one point, and a residency cutoff that excludes values
/// backed after it.
pub(crate) struct BackedSnapshot {
    pub(crate) keys: Vec<DiceKey>,
    pub(crate) cutoff: u64,
}

/// The core state with its values: what the actor thread operates on.
#[derive(Allocative)]
pub(crate) struct VersionedGraph {
    core: CoreState<DiceEnv>,
    values: KeyMap<KeyValues>,
    #[allocative(skip)]
    index: PagableIndex,
    /// Which values a page-out writes; see [`PageOutPolicy`].
    page_out_policy: PageOutPolicy,
}

impl VersionedGraph {
    pub(crate) fn new() -> Self {
        VersionedGraph {
            core: CoreState::new(),
            values: KeyMap::default(),
            index: PagableIndex::default(),
            page_out_policy: PageOutPolicy::default(),
        }
    }

    /// Sets which values a page-out writes. Only before any computed value is retained: the
    /// candidate index follows values as they arrive and is not rebuilt.
    pub(crate) fn set_page_out_policy(&mut self, policy: PageOutPolicy) {
        debug_assert_eq!(self.index.resident_count + self.index.paged_out_count, 0);
        self.page_out_policy = policy;
    }

    pub(crate) fn core(&self) -> &CoreState<DiceEnv> {
        &self.core
    }

    /// The branch's newest version.
    pub(crate) fn head(&self, branch: BranchId) -> VersionNumber {
        self.core.head(branch)
    }

    pub(crate) fn branch_count(&self) -> usize {
        self.core.branches().count()
    }

    /// A new branch whose state is `from`'s. It shares every value it resolves with the branch
    /// it was forked from.
    pub(crate) fn fork(&mut self, from: VersionNumber) -> BranchId {
        self.core.fork(from)
    }

    /// A new branch with no parent, in which nothing resolves until committed to or written for.
    pub(crate) fn new_root(&mut self) -> BranchId {
        self.core.new_root()
    }

    /// The one path through which a key's values change, so that the paging index follows.
    fn with_values<R>(&mut self, key: DiceKey, f: impl FnOnce(&mut KeyValues) -> R) -> R {
        Self::with_values_of(
            &mut self.values,
            &mut self.index,
            self.page_out_policy,
            key,
            f,
        )
    }

    fn with_values_of<R>(
        values: &mut KeyMap<KeyValues>,
        index: &mut PagableIndex,
        policy: PageOutPolicy,
        key: DiceKey,
        f: impl FnOnce(&mut KeyValues) -> R,
    ) -> R {
        let key_values = values.entry(key).or_default();
        let before = key_values.contribution(policy);
        let result = f(key_values);
        let after = key_values.contribution(policy);
        index.apply(key, before, after);
        result
    }

    /// The value named by `revision`, which some claim of `key` names and is therefore retained.
    fn stored(&self, key: DiceKey, revision: Revision) -> MaybeResident<DiceValidValue> {
        self.values
            .get(&key)
            .and_then(|kv| kv.entry(revision))
            .map(|v| v.as_maybe_resident())
            .unwrap_or_else(|| panic!("no value retained for {key:?} at {revision:?}"))
    }

    /// Commits the changes at the branch's head and returns the version they made, which is the
    /// same one if none of them had an effect.
    pub(crate) fn commit(
        &mut self,
        branch: BranchId,
        changes: impl IntoIterator<Item = (DiceKey, ChangeType, InvalidationSourcePriority)>,
    ) -> VersionNumber {
        let mut core_changes = Vec::new();
        for (key, change, priority) in changes {
            match change {
                ChangeType::Invalidate => core_changes.push(Change::Dirty {
                    key,
                    data: priority,
                }),
                ChangeType::UpdateValue(value, storage) => {
                    let revision = self.with_values(key, |kv| kv.intern_asserted(value, storage));
                    core_changes.push(Change::Assert {
                        key,
                        revision,
                        data: priority,
                    });
                }
            }
        }
        self.core.commit(branch, core_changes)
    }

    /// Deletes `branch` (see [`CoreState::delete_branch`]) and releases every value that only it
    /// referenced. The transactions still running on it are served without reuse from then on:
    /// their lookups resolve nothing and their writes are not retained.
    pub(crate) fn delete_branch(&mut self, branch: BranchId) {
        let affected = self.core.delete_branch(branch);
        self.release_unreferenced(affected);
    }

    /// Drops every value of `keys` that no claim or assertion names any more.
    fn release_unreferenced(&mut self, keys: impl IntoIterator<Item = DiceKey>) {
        let core = &self.core;
        let mut dropped = Vec::new();
        let policy = self.page_out_policy;
        for key in keys {
            Self::with_values_of(&mut self.values, &mut self.index, policy, key, |kv| {
                let (kept, unreferenced): (SmallVec<_>, SmallVec<_>) =
                    std::mem::take(&mut kv.entries)
                        .into_iter()
                        .partition(|(revision, _)| core.is_referenced(key, *revision));
                kv.entries = kept;
                dropped.extend(unreferenced);
            });
        }
        if dropped.is_empty() {
            return;
        }
        // There may be a lot to drop; do it off the actor thread.
        std::thread::Builder::new()
            .name("dice-drop-values".to_owned())
            .spawn(move || drop(dropped))
            .expect("failed to spawn thread");
    }

    pub(crate) fn get(&self, at: VersionedGraphKey) -> VersionedGraphResult {
        let (key, v) = (at.k, at.v);
        if !self.core.is_live(v.branch()) {
            // Nothing is known about a deleted branch any more, its untracked-input histories
            // included. The ε handed out here is never retained: `update` keeps nothing that a
            // transaction on a deleted branch writes.
            return VersionedGraphResult::Unknown {
                candidate: None,
                epsilon: EpsilonToken::INITIAL,
                sources: InvalidationSources::NONE,
            };
        }
        match self.core.lookup(key, v) {
            Lookup::Valid {
                revision,
                attribute,
            } => VersionedGraphResult::Match {
                value: MaybeResidentComputedValue::new(
                    self.stored(key, revision).into_payload(),
                    attribute,
                    revision,
                ),
            },
            Lookup::Unknown {
                candidate,
                epsilon,
                untracked,
            } => {
                let (cert, revalidatable) = match candidate {
                    Some(cert) => (Some(cert), true),
                    None => (self.core.nearest_certificate(key, v), false),
                };
                VersionedGraphResult::Unknown {
                    candidate: cert.map(|cert| Candidate {
                        entry: self.stored(key, cert.revision),
                        cert: cert.dupe(),
                        revalidatable,
                    }),
                    epsilon,
                    sources: untracked,
                }
            }
        }
    }

    /// Inputs for demand recovery, without returning the unreadable cached value again: the
    /// untracked-input revision to stamp the recomputed certificate with, and the sources its
    /// result starts from. Asserted keys, including ordinary `Key`s set with `changed_to`,
    /// cannot be repaired by a compute: their asserted value is authoritative and the core
    /// forbids writing a claim.
    pub(crate) fn recovery_untracked(
        &self,
        at: VersionedGraphKey,
    ) -> Option<(EpsilonToken, InvalidationSources)> {
        if self.core.is_asserted(at.k) {
            return None;
        }
        Some(if self.core.is_live(at.v.branch()) {
            self.core.untracked(at.k, at.v)
        } else {
            (EpsilonToken::INITIAL, InvalidationSources::NONE)
        })
    }

    /// Writes a value computed by a transaction at `at.v`.
    /// Returns the canonical resident instance for the revision.
    pub(crate) fn update_computed(
        &mut self,
        at: VersionedGraphKey,
        update: ComputedValueUpdate,
        invalidation_sources: InvalidationSources,
    ) -> ResidentComputedValue {
        let ComputedValueUpdate {
            value,
            mut deps,
            epsilon,
        } = update;
        let key = at.k;
        if !self.core.is_live(at.v.branch()) {
            // The core knows nothing about the branch any more, so the certificate cannot be
            // placed: the revision is minted for the value but nothing is retained.
            let revision = self.with_values(key, |kv| kv.mint.mint());
            return ResidentComputedValue::new(value, invalidation_sources, revision);
        }
        debug_assert_eq!(
            epsilon,
            self.core.epsilon(key, at.v),
            "a write's ε must be the one its transaction looked up"
        );
        let identical = self
            .core
            .pinned_certs(key)
            .find(|c| c.epsilon == epsilon && c.deps == deps)
            .cloned();
        let (cert, value) = match identical {
            Some(cert) => {
                let policy = self.page_out_policy;
                let stored = self.with_values(key, |kv| kv.adopt(cert.revision, value, policy));
                (cert, stored)
            }
            None => {
                let (revision, stored) = self.with_values(key, |kv| kv.intern_computed(value));
                deps.shrink_to_fit();
                (Arc::new(Cert::new(key, revision, deps, epsilon)), stored)
            }
        };
        let revision = cert.revision;
        let sources = self.finish_update(at, cert, invalidation_sources);
        ResidentComputedValue::new(value, sources, revision)
    }

    /// Reissues a candidate whose dependencies still hold; its value may remain paged out.
    pub(crate) fn revalidate(
        &mut self,
        at: VersionedGraphKey,
        candidate: Candidate,
        invalidation_sources: InvalidationSources,
    ) -> MaybeResidentComputedValue {
        let revision = candidate.cert.revision;
        if !self.core.is_live(at.v.branch()) {
            // The core knows nothing about the branch any more, so the certificate cannot be
            // placed: the value goes back to the transaction as it is, and nothing is retained.
            return MaybeResidentComputedValue::new(
                candidate.entry.into_payload(),
                invalidation_sources,
                revision,
            );
        }
        let policy = self.page_out_policy;
        let value = self.with_values(at.k, |kv| kv.restore(revision, candidate.entry, policy));
        let sources = self.finish_update(at, candidate.cert, invalidation_sources);
        MaybeResidentComputedValue::new(value.into_payload(), sources, revision)
    }

    /// Writes `cert` and returns the sources of the key at `at.v`: those of the claim now
    /// covering it, or `invalidation_sources`, the transaction's own, if the write installed no
    /// claim covering `at.v`.
    fn finish_update(
        &mut self,
        at: VersionedGraphKey,
        cert: Arc<DiceCert>,
        invalidation_sources: InvalidationSources,
    ) -> InvalidationSources {
        let key = at.k;
        debug_assert_eq!(
            invalidation_sources.merge(self.core.untracked(key, at.v).1),
            invalidation_sources,
            "a transaction's sources start from its key's own dirty"
        );
        self.core.write(cert);
        let core = &self.core;
        let policy = self.page_out_policy;
        Self::with_values_of(&mut self.values, &mut self.index, policy, key, |kv| {
            kv.entries.retain(|(r, _)| core.is_referenced(key, *r));
        });
        match self.core.lookup(key, at.v) {
            Lookup::Valid { attribute, .. } => attribute,
            Lookup::Unknown { .. } => invalidation_sources,
        }
    }

    /// The number of keys the state holds anything for.
    pub(crate) fn key_count(&self) -> usize {
        self.core.key_count()
    }

    /// Every resident value the next page-out serializes, with its key.
    pub(crate) fn keys_to_page_out(&self) -> Vec<(DiceKey, DiceValidValue)> {
        let mut out = Vec::with_capacity(self.index.candidate_count);
        for index in self.index.candidates.iter() {
            let key = DiceKey {
                index: index as u32,
            };
            if let Some(kv) = self.values.get(&key) {
                for value in kv.values() {
                    if value.is_page_out_candidate(self.page_out_policy) {
                        out.push((
                            key,
                            value.as_hydrated().expect("a candidate is resident").dupe(),
                        ));
                    }
                }
            }
        }
        out
    }

    /// Every paged-out value, with its key.
    pub(crate) fn paged_out_keys(&self) -> Vec<(DiceKey, DataKey)> {
        let mut out = Vec::with_capacity(self.index.paged_out_count);
        for (key, kv) in &self.values {
            if kv.injected {
                continue;
            }
            for value in kv.values() {
                if let Some(data_key) = value.data_key() {
                    out.push((*key, data_key));
                }
            }
        }
        out
    }

    /// The keys of every resident, every paged-out, and every backed value that takes part in
    /// paging, a key once per value; the backed keys are also among the resident ones.
    pub(crate) fn keys_by_paging_state(&self) -> (Vec<DiceKey>, Vec<DiceKey>, Vec<DiceKey>) {
        let mut resident = Vec::with_capacity(self.index.resident_count);
        let mut paged_out = Vec::with_capacity(self.index.paged_out_count);
        let mut backed = Vec::with_capacity(self.index.backed_count);
        for (key, kv) in &self.values {
            if kv.injected {
                continue;
            }
            for value in kv.values() {
                match value {
                    PagableValue::PagedOut(_) => paged_out.push(*key),
                    PagableValue::Backed(_) => {
                        resident.push(*key);
                        backed.push(*key);
                    }
                    _ => resident.push(*key),
                }
            }
        }
        (resident, paged_out, backed)
    }

    pub(crate) fn pagable_node_counts(&self) -> PagableNodeCounts {
        PagableNodeCounts {
            resident: self.index.resident_count,
            paged_out: self.index.paged_out_count,
            candidates: self.index.candidate_count,
            backed: self.index.backed_count,
            backed_bytes: self.index.backed_bytes,
        }
    }

    /// Records the row of each value that is still the exact allocation page-out serialized: a
    /// value never paged out is evicted, a recomputed one stays resident, backed by its row. A
    /// value that a write replaced in the meantime is left alone. Returns how many values were
    /// evicted.
    pub(crate) fn commit_page_out(
        &mut self,
        keys: Vec<(DiceKey, PageOutResult)>,
        eviction_policy: EvictionPolicy,
    ) -> u64 {
        let mut evicted = 0u64;
        for (
            key,
            PageOutResult {
                serialized_value,
                data_key,
                serialized_bytes,
            },
        ) in keys
        {
            self.with_values(key, |kv| {
                for value in kv.values_mut() {
                    if value
                        .as_hydrated()
                        .is_some_and(|current| current.ptr_eq(&serialized_value))
                    {
                        if value.row_committed(data_key, serialized_bytes, eviction_policy)
                            == RowCommitted::Evicted
                        {
                            evicted += 1;
                        }
                    }
                }
            });
        }
        evicted
    }

    /// What is backed now, for an eviction sweep that must leave alone values backed after it
    /// starts.
    pub(crate) fn backed_snapshot(&self) -> BackedSnapshot {
        BackedSnapshot {
            keys: self
                .index
                .backed
                .iter()
                .map(|index| DiceKey {
                    index: index as u32,
                })
                .collect(),
            cutoff: lifecycle::residency_cutoff(),
        }
    }

    /// Evicts the values of `keys` backed no later than `cutoff`,
    /// leaving their rows. Returns how many values were evicted.
    pub(crate) fn evict_backed(&mut self, keys: &[DiceKey], cutoff: u64) -> u64 {
        let mut evicted = 0u64;
        for &key in keys {
            self.with_values(key, |kv| {
                for value in kv.values_mut() {
                    if value.evict_backed(cutoff) {
                        evicted += 1;
                    }
                }
            });
        }
        evicted
    }

    /// Marks each value page-out could not serialize, if it is still the exact allocation
    /// inspected, so that it is not offered as a candidate again.
    pub(crate) fn mark_non_pageable(&mut self, keys: Vec<(DiceKey, DiceValidValue)>) {
        for (key, inspected) in keys {
            self.with_values(key, |kv| {
                for value in kv.values_mut() {
                    if value
                        .as_hydrated()
                        .is_some_and(|current| current.ptr_eq(&inspected))
                    {
                        *value = PagableValue::NonPageable(inspected.dupe());
                    }
                }
            });
        }
    }

    /// Makes the value paged out at `data_key`, read back from a row of `serialized_bytes`,
    /// resident again. No-op if no such value is retained any more.
    pub(crate) fn rehydrate(
        &mut self,
        key: DiceKey,
        data_key: DataKey,
        value: DiceValidValue,
        serialized_bytes: u64,
    ) {
        let policy = self.page_out_policy;
        self.with_values(key, |kv| {
            if let Some((_, stored)) = kv
                .entries
                .iter_mut()
                .find(|(_, v)| v.data_key() == Some(data_key))
            {
                stored.make_resident(value, serialized_bytes, policy);
            }
        });
    }

    /// Asserts, by full scan, that the paging index matches the values, that the core's own
    /// invariants hold, and that values and claims reference each other exactly: every
    /// certificate a claim pins has its value retained, and every value retained is named by
    /// some claim or assertion.
    #[cfg(test)]
    pub(crate) fn assert_consistent(&self) {
        self.core.check_invariants();
        let mut total = Contribution::default();
        let mut candidates = Vec::new();
        let mut backed = Vec::new();
        for (key, kv) in &self.values {
            let c = kv.contribution(self.page_out_policy);
            total.resident += c.resident;
            total.paged_out += c.paged_out;
            total.candidates += c.candidates;
            total.backed += c.backed;
            total.backed_bytes += c.backed_bytes;
            if c.candidates > 0 {
                candidates.push(key.index as usize);
            }
            if c.backed > 0 {
                backed.push(key.index as usize);
            }
            for cert in self.core.pinned_certs(*key) {
                assert!(
                    kv.entry(cert.revision).is_some(),
                    "{key:?} pins a certificate at {:?} but retains no value for it",
                    cert.revision
                );
            }
            for (revision, _) in &kv.entries {
                assert!(
                    self.core.is_referenced(*key, *revision),
                    "{key:?} retains {revision:?}, which no claim or assertion names"
                );
            }
        }
        candidates.sort_unstable();
        backed.sort_unstable();
        assert_eq!(self.index.candidates.iter().collect::<Vec<_>>(), candidates);
        assert_eq!(self.index.candidate_count, total.candidates);
        assert_eq!(self.index.backed.iter().collect::<Vec<_>>(), backed);
        assert_eq!(self.index.backed_count, total.backed);
        assert_eq!(self.index.backed_bytes, total.backed_bytes);
        assert_eq!(self.index.resident_count, total.resident);
        assert_eq!(self.index.paged_out_count, total.paged_out);
    }
}
