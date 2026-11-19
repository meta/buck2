/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::Weak;

use allocative::Allocative;
use dupe::Dupe;

use crate::PagableDeserializer;
use crate::PagableSerialize;
use crate::PagableSerializer;
use crate::arc_erase::ArcErase;
use crate::arc_erase::ArcEraseType;
use crate::arc_erase::ArcSerializeOutcome;
use crate::arc_erase::StdArcEraseType;
use crate::arc_erase::WeakErase;
use crate::storage::data::DataKey;

/// An `Arc` for values that may be only partially materialized after
/// deserialization.
///
/// The in-memory value remains resident and is completed lazily as parts are
/// accessed. Once associated with a `DataKey`, future page-out traversals reuse
/// that key instead of serializing the partial in-memory representation.
#[derive(Allocative)]
pub struct PartialPagableArc<T> {
    inner: Arc<PartialPagableArcInner<T>>,
}

#[derive(Allocative)]
struct PartialPagableArcInner<T> {
    value: T,
    data_key: OnceLock<DataKey>,
}

impl<T> PartialPagableArc<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(PartialPagableArcInner {
                value,
                data_key: OnceLock::new(),
            }),
        }
    }

    pub fn data_key(&self) -> Option<DataKey> {
        self.inner.data_key.get().copied()
    }

    pub fn as_ptr(&self) -> *const T {
        &self.inner.value
    }

    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        Arc::ptr_eq(&this.inner, &other.inner)
    }

    pub fn downgrade(this: &Self) -> PartialPagableWeak<T> {
        PartialPagableWeak {
            inner: Arc::downgrade(&this.inner),
        }
    }

    fn set_data_key(&self, key: DataKey) {
        let existing = self.inner.data_key.get_or_init(|| key);
        assert_eq!(
            *existing, key,
            "a PartialPagableArc cannot refer to two stored representations",
        );
    }
}

impl<T> Clone for PartialPagableArc<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T> Dupe for PartialPagableArc<T> {}

impl<T> Deref for PartialPagableArc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner.value
    }
}

impl<T: fmt::Debug> fmt::Debug for PartialPagableArc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PartialPagableArc")
            .field(&self.inner.value)
            .finish()
    }
}

#[derive(Allocative)]
pub struct PartialPagableWeak<T> {
    #[allocative(skip)]
    inner: Weak<PartialPagableArcInner<T>>,
}

impl<T> Clone for PartialPagableWeak<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T> Dupe for PartialPagableWeak<T> {}

impl<T> PartialPagableWeak<T> {
    pub fn upgrade(&self) -> Option<PartialPagableArc<T>> {
        self.inner
            .upgrade()
            .map(|inner| PartialPagableArc { inner })
    }

    pub fn as_ptr(&self) -> *const T {
        let inner = self.inner.as_ptr();
        if inner.is_null() {
            return std::ptr::null();
        }
        // SAFETY: `Weak::as_ptr` remains a valid address for pointer comparison
        // after the strong allocation has been dropped; no memory is dereferenced.
        unsafe { std::ptr::addr_of!((*inner).value) }
    }
}

impl<T> PagableSerialize for PartialPagableArc<T>
where
    T: PagableSerialize + Send + Sync + 'static,
{
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        serializer.serialize_arc(self)
    }
}

impl<T> ArcErase for PartialPagableArc<T>
where
    T: PagableSerialize + Send + Sync + 'static,
{
    type Weak = PartialPagableWeak<T>;

    fn dupe_strong(&self) -> Self {
        self.dupe()
    }

    fn downgrade(&self) -> Option<Self::Weak> {
        Some(Self::downgrade(self))
    }

    fn erase_type() -> impl ArcEraseType {
        StdArcEraseType::<Self>::new()
    }

    fn identity(&self) -> usize {
        self.as_ptr() as usize
    }

    fn set_data_key(&self, key: DataKey) {
        self.set_data_key(key);
    }

    fn data_key(&self) -> Option<DataKey> {
        self.data_key()
    }

    fn needs_paging_out(&self) -> bool {
        self.data_key().is_none()
    }

    fn serialize_inner(
        &self,
        serializer: &mut dyn PagableSerializer,
    ) -> crate::Result<ArcSerializeOutcome> {
        if let Some(key) = self.data_key() {
            return Ok(ArcSerializeOutcome::ReuseDataKey(key));
        }
        self.inner.value.pagable_serialize(serializer)?;
        Ok(ArcSerializeOutcome::Serialized)
    }

    fn deserialize_inner<'de, D: PagableDeserializer<'de> + ?Sized>(
        _deserializer: &mut D,
    ) -> crate::Result<Self> {
        Err(anyhow::anyhow!(
            "PartialPagableArc requires a recipe-aware deserialization callback",
        ))
    }
}

impl<T> WeakErase for PartialPagableWeak<T>
where
    T: PagableSerialize + Send + Sync + 'static,
{
    fn is_expired(&self) -> bool {
        self.inner.strong_count() == 0
    }

    fn upgrade_weak(&self) -> Option<Box<dyn crate::arc_erase::ArcEraseDyn>> {
        self.upgrade().map(|arc| Box::new(arc) as _)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_the_original_data_key() {
        let arc = PartialPagableArc::new(17u8);
        let clone = arc.dupe();
        let key = DataKey::testing_new(1);

        ArcErase::set_data_key(&arc, key);

        assert_eq!(arc.data_key(), Some(key));
        assert_eq!(clone.data_key(), Some(key));
        assert!(PartialPagableArc::ptr_eq(&arc, &clone));
    }
}
