/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::any::Any;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::sync::Arc as StdArc;

use allocative::Allocative;
use dice_error::DiceResult;
use dupe::Dupe;
use mini_vec::packed_ptr::PackedPtr;
use pagable::DataKey;
use tokio::sync::OnceCell;

use crate::Key;
use crate::ProjectionKey;
use crate::api::key::EqualityBehavior;
use crate::api::key::InvalidationSourcePriority;
use crate::arc::Arc;
use crate::core::graph::revision::EpsilonToken;
use crate::core::graph::revision::Revision;
use crate::epoch::task::dice::DiceTask;
use crate::key::DiceKey;
use crate::versions::VersionNumber;

/// Type erased value associated for each Key in Dice. The 'DiceValidValue' only holds valid values
/// and never anything that is transient, or whose dependencies are transient.
#[derive(Allocative, Clone, Dupe)]
pub(crate) struct DiceValidValue(std::sync::Arc<dyn DiceValueDyn>);

impl Debug for DiceValidValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiceValue").finish_non_exhaustive()
    }
}

impl DiceValidValue {
    #[cfg(test)]
    pub(crate) fn downcast_ref<V: Any>(&self) -> Option<&V> {
        self.0.downcast_ref()
    }

    /// Dynamic version of `Key::equality`.
    pub(crate) fn equality(&self, other: &DiceValidValue) -> bool {
        self.0.equality(&*other.0)
    }

    pub(crate) fn as_dyn(&self) -> &dyn DiceValueDyn {
        &*self.0
    }

    pub(crate) fn from_arc(arc: std::sync::Arc<dyn DiceValueDyn>) -> Self {
        Self(arc)
    }

    /// Whether both wrappers own the same erased value allocation.
    #[allow(ambiguous_wide_pointer_comparisons)]
    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Successful serialization of one page-out candidate.
///
/// `serialized_value` keeps the exact allocation alive so the core can reject
/// this result if the graph installs a different value before serialization finishes.
pub(crate) struct PageOutResult {
    pub(crate) serialized_value: DiceValidValue,
    pub(crate) data_key: DataKey,
    /// Size of the top-level serialized blob, excluding nested `PagableArc` sub-values.
    pub(crate) serialized_bytes: u64,
}

/// A value read back from its row.
pub(crate) struct HydratedValue {
    pub(crate) value: DiceValidValue,
    /// Size of the row's top-level serialized blob, excluding nested `PagableArc` sub-values.
    pub(crate) serialized_bytes: u64,
}

/// Type erased value that may be transient, or whose dependencies are transient
#[derive(Allocative, Clone, Dupe)]
pub enum MaybeValidDiceValue {
    Present(StdArc<dyn DiceValueDyn>),
    Transient(StdArc<dyn DiceValueDyn>),
}

impl MaybeValidDiceValue {
    pub(crate) fn new(value: StdArc<dyn DiceValueDyn>, deps_validity: DiceValidity) -> Self {
        let validity = if value.validity() {
            deps_validity
        } else {
            DiceValidity::Transient
        };

        match validity {
            DiceValidity::Valid => Self::Present(value),
            DiceValidity::Transient => Self::Transient(value),
        }
    }

    pub(crate) fn valid(value: DiceValidValue) -> Self {
        Self::Present(value.0)
    }

    pub(crate) fn validity(&self) -> DiceValidity {
        match self {
            Self::Transient(_) => DiceValidity::Transient,
            _ => DiceValidity::Valid,
        }
    }

    fn value(&self) -> &std::sync::Arc<dyn DiceValueDyn> {
        match self {
            MaybeValidDiceValue::Present(v) | MaybeValidDiceValue::Transient(v) => v,
        }
    }

    pub(crate) fn downcast_maybe_transient<V: Any>(&self) -> Option<&V> {
        self.value().downcast_ref()
    }

    /// Dynamic version of `Key::equality`.
    #[cfg(test)]
    pub(crate) fn equality(&self, other: &DiceValidValue) -> bool {
        self.value().equality(&*other.0)
    }

    #[cfg(test)]
    pub(crate) fn instance_equal(&self, other: &DiceValidValue) -> bool {
        #[allow(ambiguous_wide_pointer_comparisons)]
        // we literally just want to compare the exact pointer
        std::sync::Arc::ptr_eq(self.value(), &other.0)
    }

    pub(crate) fn into_valid_value(self) -> Result<DiceValidValue, MaybeValidDiceValue> {
        match self {
            Self::Present(v) => Ok(DiceValidValue(v)),
            v @ Self::Transient(_) => Err(v),
        }
    }
}

/// A DICE value that may not be in memory: either resident, or paged out to storage, where
/// `DataKey` locates it.
///
/// A paged-out value is read back only when a caller needs the payload. Everything a
/// dependency check needs, the value's revision and its invalidation paths, travels beside the
/// value, so proving a key unchanged never has to touch storage.
#[derive(Allocative, Clone, Dupe)]
pub(crate) enum MaybeResident<V> {
    Resident(V),
    PagedOut(DataKey),
}

impl<V> MaybeResident<V> {
    /// The on-disk key to read the value back from, or `None` if it is already resident.
    pub(crate) fn data_key(&self) -> Option<DataKey> {
        match self {
            MaybeResident::Resident(_) => None,
            MaybeResident::PagedOut(data_key) => Some(*data_key),
        }
    }
}

impl MaybeResident<DiceValidValue> {
    /// The same value as a [`MaybeResidentComputedValue`] payload.
    pub(crate) fn into_payload(self) -> MaybeResident<MaybeValidDiceValue> {
        match self {
            MaybeResident::Resident(v) => MaybeResident::Resident(MaybeValidDiceValue::valid(v)),
            MaybeResident::PagedOut(data_key) => MaybeResident::PagedOut(data_key),
        }
    }
}

/// Deliberately does not require `V: Debug`: neither `DiceValidValue` nor
/// `MaybeValidDiceValue` prints its contents anyway.
impl<V> Debug for MaybeResident<V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            MaybeResident::Resident(_) => f.write_str("Resident(..)"),
            MaybeResident::PagedOut(data_key) => write!(f, "PagedOut({data_key:?})"),
        }
    }
}

/// validity, including based on validity of dependencies
#[derive(Allocative, Clone, Dupe, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DiceValidity {
    Valid,
    /// If the node is invalid or any of its deps are invalid
    Transient,
}

impl DiceValidity {
    pub(crate) fn and(&mut self, other: Self) {
        if other == DiceValidity::Transient {
            *self = DiceValidity::Transient;
        }
    }
}

#[derive(Allocative, Clone, Dupe)]
pub(crate) struct ComputedValue<V> {
    value: V,
    invalidation_paths: TrackedInvalidationPaths,
    /// The revision the value was interned under, `None` iff the value is transient:
    /// transients never enter the graph and so have no interned identity to name.
    revision: Option<Revision>,
}

pub(crate) type MaybeResidentComputedValue = ComputedValue<MaybeResidentDiceValue>;
pub(crate) type ResidentComputedValue = ComputedValue<MaybeValidDiceValue>;

/// Borrows a resident payload and its matching metadata from a task or page-in result.
#[derive(Clone, Copy, Dupe)]
pub(crate) struct ResidentComputedValueRef<'d> {
    value: &'d MaybeValidDiceValue,
    invalidation_paths: &'d TrackedInvalidationPaths,
    revision: Option<Revision>,
}

impl ResidentComputedValue {
    pub(crate) fn new(
        value: DiceValidValue,
        invalidation_paths: TrackedInvalidationPaths,
        revision: Revision,
    ) -> Self {
        Self {
            value: MaybeValidDiceValue::valid(value),
            invalidation_paths,
            revision: Some(revision),
        }
    }

    pub(crate) fn new_for_transient(
        value: MaybeValidDiceValue,
        invalidation_paths: TrackedInvalidationPaths,
    ) -> Self {
        Self {
            value,
            invalidation_paths,
            revision: None,
        }
    }

    pub(crate) fn into_computed(self) -> MaybeResidentComputedValue {
        MaybeResidentComputedValue {
            value: MaybeResidentDiceValue::Resident(self.value),
            invalidation_paths: self.invalidation_paths,
            revision: self.revision,
        }
    }

    pub(crate) fn as_ref(&self) -> ResidentComputedValueRef<'_> {
        ResidentComputedValueRef {
            value: &self.value,
            invalidation_paths: &self.invalidation_paths,
            revision: self.revision,
        }
    }
}

impl<'d> ResidentComputedValueRef<'d> {
    pub(crate) fn value(&self) -> &'d MaybeValidDiceValue {
        self.value
    }

    pub(crate) fn invalidation_paths(&self) -> &'d TrackedInvalidationPaths {
        self.invalidation_paths
    }

    pub(crate) fn revision(&self) -> Option<Revision> {
        self.revision
    }
}

/// Payload owned by an epoch result. Copies share the paged-out descriptor,
/// keeping its identity and paging state tied to the result.
#[derive(Allocative, Clone, Dupe)]
pub(crate) enum MaybeResidentDiceValue {
    Resident(MaybeValidDiceValue),
    PagedOut(Arc<PagedOutValue>),
}

impl MaybeResidentDiceValue {
    fn new(value: MaybeResident<MaybeValidDiceValue>) -> Self {
        match value {
            MaybeResident::Resident(value) => Self::Resident(value),
            MaybeResident::PagedOut(data_key) => Self::PagedOut(Arc::new(PagedOutValue {
                data_key,
                outcome: OnceCell::new(),
            })),
        }
    }

    fn resident_value(&self) -> Option<&MaybeValidDiceValue> {
        match self {
            Self::Resident(value) => Some(value),
            Self::PagedOut(_) => None,
        }
    }
}

/// Shared by every copy of a paged-out task result in an active-version cache. Reads and
/// recovery are coalesced, and their results stay alive for references borrowed from the task.
#[derive(Allocative)]
pub(crate) struct PagedOutValue {
    pub(crate) data_key: DataKey,
    #[allocative(skip)]
    pub(crate) outcome: OnceCell<PageInOutcome>,
}

#[derive(Allocative)]
pub(crate) enum PageInOutcome {
    Read(DiceResult<ResidentComputedValue>),
    Recompute(PageInRecovery),
}

/// Owned by the paged-out descriptor, not a second task registry. The ordinary task's result
/// keeps this reachable for both subsequent lookups and cache draining.
#[derive(Allocative)]
pub(crate) struct PageInRecovery {
    pub(crate) task: DiceTask,
    pub(crate) epsilon: EpsilonToken,
}

#[derive(Allocative, Debug, Clone, Dupe, PartialEq, Eq)]
pub(crate) enum InvalidationPath {
    Clean,
    Unknown,
    Invalidated(Arc<InvalidationPathNode>),
}

impl InvalidationPath {
    fn for_dependent(&self, key: DiceKey) -> InvalidationPath {
        match self {
            InvalidationPath::Clean => InvalidationPath::Clean,
            InvalidationPath::Unknown => InvalidationPath::Unknown,
            InvalidationPath::Invalidated(v) => {
                InvalidationPath::Invalidated(Arc::new(InvalidationPathNode {
                    key,
                    version: v.version,
                    cause: self.dupe(),
                }))
            }
        }
    }

    fn at_version(&self, v: VersionNumber) -> InvalidationPath {
        match self {
            InvalidationPath::Invalidated(t)
                if t.version.branch() != v.branch() || t.version.seq() > v.seq() =>
            {
                InvalidationPath::Unknown
            }
            _ => self.dupe(),
        }
    }

    /// Always returns one of its arguments; callers rely on that to avoid reallocating when the
    /// merge is a no-op.
    fn merge<'a>(&'a self, other: &'a InvalidationPath) -> &'a InvalidationPath {
        match (self, other) {
            (InvalidationPath::Unknown, _) => self,
            // FIXME(JakobDegen): Why the asymmetric treatment? Bug?
            (_, InvalidationPath::Unknown) => self,
            (InvalidationPath::Clean, _) => other,
            (_, InvalidationPath::Clean) => self,
            (
                InvalidationPath::Invalidated(self_node),
                InvalidationPath::Invalidated(other_node),
            ) => {
                if self_node.version.cmp_on_chain(other_node.version).is_ge() {
                    self
                } else {
                    other
                }
            }
        }
    }
}

#[derive(Allocative, Debug, Clone, Dupe, PartialEq, Eq)]
pub(crate) struct InvalidationPathNode {
    /// The key at this node in the path.
    pub(crate) key: DiceKey,
    pub(crate) version: VersionNumber,
    pub(crate) cause: InvalidationPath,
}

#[derive(Allocative, Debug, Clone, Dupe, PartialEq, Eq)]
struct InvalidationPathsHeap {
    normal: InvalidationPath,
    high: InvalidationPath,
}

const TAG_ALLOCATED: u8 = 0;
const TAG_BOTH_CLEAN: u8 = 1;
const TAG_BOTH_UNKNOWN: u8 = 2;

#[derive(Allocative, Debug, Clone, Dupe, PartialEq, Eq)]
pub(crate) struct TrackedInvalidationPaths(PackedPtr<Option<Arc<InvalidationPathsHeap>>>);

impl TrackedInvalidationPaths {
    fn from_pair(normal: InvalidationPath, high: InvalidationPath) -> Self {
        match (normal, high) {
            (InvalidationPath::Clean, InvalidationPath::Clean) => {
                Self(PackedPtr::new(None, TAG_BOTH_CLEAN))
            }
            (InvalidationPath::Unknown, InvalidationPath::Unknown) => {
                Self(PackedPtr::new(None, TAG_BOTH_UNKNOWN))
            }
            (normal, high) => Self(PackedPtr::new(
                Some(Arc::new(InvalidationPathsHeap { normal, high })),
                TAG_ALLOCATED,
            )),
        }
    }

    fn get(&self) -> (&InvalidationPath, &InvalidationPath) {
        match self.0.extra() {
            TAG_ALLOCATED => {
                let h = self.0.as_ref().unwrap();
                (&h.normal, &h.high)
            }
            TAG_BOTH_CLEAN => (&InvalidationPath::Clean, &InvalidationPath::Clean),
            TAG_BOTH_UNKNOWN => (&InvalidationPath::Unknown, &InvalidationPath::Unknown),
            _ => unreachable!(),
        }
    }

    pub(crate) fn for_dependent(&self, key: DiceKey) -> TrackedInvalidationPaths {
        let (normal, high) = self.get();
        TrackedInvalidationPaths::from_pair(normal.for_dependent(key), high.for_dependent(key))
    }

    pub(crate) fn clean() -> TrackedInvalidationPaths {
        Self::from_pair(InvalidationPath::Clean, InvalidationPath::Clean)
    }

    pub(crate) fn at_version(&self, v: VersionNumber) -> TrackedInvalidationPaths {
        let (normal, high) = self.get();
        TrackedInvalidationPaths::from_pair(normal.at_version(v), high.at_version(v))
    }

    pub(crate) fn new(
        priority: InvalidationSourcePriority,
        key: DiceKey,
        version: VersionNumber,
    ) -> Self {
        let path = InvalidationPath::Invalidated(Arc::new(InvalidationPathNode {
            key,
            version,
            cause: InvalidationPath::Clean,
        }));
        match priority {
            InvalidationSourcePriority::Ignored => Self::clean(),
            InvalidationSourcePriority::Normal => Self::from_pair(path, InvalidationPath::Clean),
            InvalidationSourcePriority::High => Self::from_pair(path.dupe(), path),
        }
    }

    pub(crate) fn update(&mut self, new_paths: &TrackedInvalidationPaths) {
        let (this_normal, this_high) = self.get();
        let (new_normal, new_high) = new_paths.get();
        let normal = this_normal.merge(new_normal);
        let high = this_high.merge(new_high);
        // This runs once per recorded dependency, and dependency-heavy keys record hundreds of
        // thousands, so avoid reallocating the heap in the common cases where one side wins both
        // merges. `merge` always returns one of its arguments, so pointer identity detects that.
        if std::ptr::eq(normal, this_normal) && std::ptr::eq(high, this_high) {
            return;
        }
        if std::ptr::eq(normal, new_normal) && std::ptr::eq(high, new_high) {
            *self = new_paths.dupe();
            return;
        }
        *self = Self::from_pair(normal.dupe(), high.dupe());
    }

    pub(crate) fn get_normal(&self) -> InvalidationPath {
        self.get().0.dupe()
    }

    pub(crate) fn get_high(&self) -> InvalidationPath {
        self.get().1.dupe()
    }
}

impl MaybeResidentComputedValue {
    pub(crate) fn new(
        value: MaybeResident<MaybeValidDiceValue>,
        invalidation_paths: TrackedInvalidationPaths,
        revision: Revision,
    ) -> Self {
        Self {
            value: MaybeResidentDiceValue::new(value),
            invalidation_paths,
            revision: Some(revision),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_resident(
        value: MaybeValidDiceValue,
        invalidation_paths: TrackedInvalidationPaths,
        revision: Revision,
    ) -> Self {
        Self::new(MaybeResident::Resident(value), invalidation_paths, revision)
    }

    /// The same result with `value` — read back from storage — as its payload.
    pub(crate) fn paged_in(&self, value: DiceValidValue) -> ResidentComputedValue {
        ResidentComputedValue {
            value: MaybeValidDiceValue::valid(value),
            invalidation_paths: self.invalidation_paths.dupe(),
            revision: self.revision,
        }
    }

    /// A transient result: never stored in the core state and so without a revision.
    pub(crate) fn new_for_transient(
        value: MaybeValidDiceValue,
        invalidation_paths: TrackedInvalidationPaths,
    ) -> Self {
        ResidentComputedValue::new_for_transient(value, invalidation_paths).into_computed()
    }

    /// Borrow the original resident representation, without following page-in outcomes.
    /// Use `try_as_resident` to also borrow successfully hydrated values.
    pub(crate) fn resident_value(&self) -> Option<&MaybeValidDiceValue> {
        self.value.resident_value()
    }

    /// Borrow an original or successfully hydrated result. Recovery is followed separately
    /// because it can change the result's revision.
    pub(crate) fn try_as_resident(&self) -> Result<ResidentComputedValueRef<'_>, &PagedOutValue> {
        match &self.value {
            MaybeResidentDiceValue::Resident(value) => Ok(ResidentComputedValueRef {
                value,
                invalidation_paths: &self.invalidation_paths,
                revision: self.revision,
            }),
            MaybeResidentDiceValue::PagedOut(paged_out) => match paged_out.outcome.get() {
                Some(PageInOutcome::Read(Ok(value))) => Ok(value.as_ref()),
                _ => Err(paged_out),
            },
        }
    }

    /// The on-disk key to read the value back from, or `None` if it is already resident.
    #[cfg(test)]
    pub(crate) fn paged_out_data_key(&self) -> Option<DataKey> {
        self.try_as_resident().err().map(|value| value.data_key)
    }

    /// Whether the payload can be borrowed without reading storage.
    pub(crate) fn has_resident_value(&self) -> bool {
        match self.try_as_resident() {
            Ok(_) => true,
            Err(paged_out) => match paged_out.outcome.get() {
                Some(PageInOutcome::Read(Ok(_))) => true,
                Some(PageInOutcome::Recompute(recovery)) => {
                    recovery.task.as_ref().get_finished_value().is_some()
                }
                _ => false,
            },
        }
    }

    /// Recovery can produce a different revision or a transient value. Later dependency
    /// checks must follow it rather than continuing to use this descriptor's old metadata.
    pub(crate) fn recovery(&self) -> Option<&PageInRecovery> {
        match self.try_as_resident().err()?.outcome.get()? {
            PageInOutcome::Recompute(recovery) => Some(recovery),
            PageInOutcome::Read(_) => None,
        }
    }
}

impl<V> ComputedValue<V> {
    pub(crate) fn invalidation_paths(&self) -> &TrackedInvalidationPaths {
        &self.invalidation_paths
    }

    /// The revision the value was interned under, `None` iff the value is transient.
    pub(crate) fn revision(&self) -> Option<Revision> {
        self.revision
    }
}

impl<V> Debug for ComputedValue<V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComputedValue")
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

pub trait DiceValueDyn: Allocative + Any + Send + Sync + 'static {
    fn value_as_any(&self) -> &dyn Any;
    /// For `Compare` values, panics if called with an incompatible value.
    fn equality(&self, other: &dyn DiceValueDyn) -> bool;
    fn validity(&self) -> bool;
}

impl dyn DiceValueDyn {
    pub(crate) fn downcast_ref<V: Any>(&self) -> Option<&V> {
        self.value_as_any().downcast_ref()
    }
}

#[derive(Allocative)]
pub(crate) struct DiceKeyValue<K: Key> {
    value: K::Value,
}

impl<K> DiceKeyValue<K>
where
    K: Key,
{
    pub(crate) fn new(value: K::Value) -> Self {
        Self { value }
    }
}

impl<K> DiceValueDyn for DiceKeyValue<K>
where
    K: Key,
{
    fn value_as_any(&self) -> &dyn Any {
        &self.value
    }

    fn equality(&self, other: &dyn DiceValueDyn) -> bool {
        match K::equality_behavior() {
            EqualityBehavior::Compare(compare) => compare(
                &self.value,
                other
                    .downcast_ref()
                    .expect("values for the same DICE key must have the same type"),
            ),
            EqualityBehavior::AlwaysUnequal => false,
        }
    }

    fn validity(&self) -> bool {
        K::validity(&self.value)
    }
}

#[derive(Allocative)]
pub(crate) struct DiceProjectValue<K: ProjectionKey> {
    value: K::Value,
}

impl<K> DiceProjectValue<K>
where
    K: ProjectionKey,
{
    pub(crate) fn new(value: K::Value) -> Self {
        Self { value }
    }
}

impl<K> DiceValueDyn for DiceProjectValue<K>
where
    K: ProjectionKey,
{
    fn value_as_any(&self) -> &dyn Any {
        &self.value
    }

    fn equality(&self, other: &dyn DiceValueDyn) -> bool {
        match K::equality_behavior() {
            EqualityBehavior::Compare(compare) => compare(
                &self.value,
                other
                    .downcast_ref()
                    .expect("values for the same DICE projection must have the same type"),
            ),
            EqualityBehavior::AlwaysUnequal => false,
        }
    }

    fn validity(&self) -> bool {
        K::validity(&self.value)
    }
}

#[cfg(test)]
pub mod testing {
    use crate::arc::Arc;
    use crate::key::DiceKey;
    use crate::value::DiceValidValue;
    use crate::value::DiceValueDyn;
    use crate::value::InvalidationPath;
    use crate::value::InvalidationPathNode;
    use crate::value::MaybeResidentComputedValue;
    use crate::value::MaybeValidDiceValue;
    use crate::value::TrackedInvalidationPaths;
    use crate::versions::VersionNumber;

    impl DiceValidValue {
        pub(crate) fn testing_new<V: DiceValueDyn>(value: V) -> Self {
            Self(std::sync::Arc::new(value))
        }
    }

    impl MaybeValidDiceValue {
        pub(crate) fn testing_value(&self) -> &std::sync::Arc<dyn DiceValueDyn> {
            self.value()
        }
    }

    impl MaybeResidentComputedValue {
        /// The payload, panicking if the value is paged out.
        pub(crate) fn testing_resident_value(&self) -> &MaybeValidDiceValue {
            self.resident_value().expect("value should be resident")
        }
    }

    pub struct MakeInvalidationPaths {
        pub normal: (DiceKey, usize),
        pub high: Option<(DiceKey, usize)>,
    }

    impl MakeInvalidationPaths {
        pub fn into(self) -> TrackedInvalidationPaths {
            let normal = InvalidationPath::Invalidated(Arc::new(InvalidationPathNode {
                key: self.normal.0,
                version: VersionNumber::testing_new(self.normal.1 as u32),
                cause: InvalidationPath::Clean,
            }));
            let high = self.high.map_or(InvalidationPath::Clean, |(k, v)| {
                InvalidationPath::Invalidated(Arc::new(InvalidationPathNode {
                    key: k,
                    version: VersionNumber::testing_new(v as u32),
                    cause: InvalidationPath::Clean,
                }))
            });
            TrackedInvalidationPaths::from_pair(normal, high)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::testing::MakeInvalidationPaths;

    #[test]

    fn test_invalidation_paths() -> anyhow::Result<()> {
        let key0 = DiceKey { index: 0 };
        let key1 = DiceKey { index: 1 };
        let key2 = DiceKey { index: 2 };
        let mut paths = TrackedInvalidationPaths::clean();

        paths.update(
            &MakeInvalidationPaths {
                normal: (key0, 2),
                high: None,
            }
            .into(),
        );

        assert_eq!(
            paths,
            MakeInvalidationPaths {
                normal: (key0, 2),
                high: None,
            }
            .into()
        );

        paths.update(
            &MakeInvalidationPaths {
                normal: (key1, 3),
                high: Some((key1, 3)),
            }
            .into(),
        );

        assert_eq!(
            paths,
            MakeInvalidationPaths {
                normal: (key1, 3),
                high: Some((key1, 3)),
            }
            .into()
        );

        paths.update(
            &MakeInvalidationPaths {
                normal: (key2, 4),
                high: None,
            }
            .into(),
        );

        assert_eq!(
            paths,
            MakeInvalidationPaths {
                normal: (key2, 4),
                high: Some((key1, 3))
            }
            .into()
        );

        Ok(())
    }
}
