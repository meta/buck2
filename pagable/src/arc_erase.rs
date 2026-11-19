/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Type erasure traits for Arc serialization.
//!
//! This module provides traits for working with type-erased Arcs, enabling
//! the pagable framework to serialize and deserialize Arc types while
//! preserving their identity (pointer equality) across serialization boundaries.
//!
//! The key traits are:
//! - [`ArcErase`] - implemented by Arc types to provide serialization capabilities
//! - [`ArcEraseDyn`] - object-safe version for dynamic dispatch over heterogeneous Arcs
//! - [`WeakErase`] / [`WeakEraseDyn`] - corresponding traits for weak references
//! - [`StrongAsWeak`] - what the erased view holds for an Arc type that has no weak reference

use std::any::TypeId;
use std::sync::Arc;

use dupe::Dupe;

use crate::PagableBoxDeserialize;
use crate::PagableDeserialize;
use crate::PagableDeserializer;
use crate::PagableDeserializerRecipe;
use crate::PagableSerialize;
use crate::PagableSerializer;
use crate::storage::data::DataKey;

/// Outcome of preparing an Arc payload for page-out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArcSerializeOutcome {
    /// The payload was written to the supplied serializer.
    Serialized,
    /// The Arc already has a canonical stored representation that must be reused.
    ReuseDataKey(DataKey),
}

/// Object-safe trait for type-erased Arc handling.
///
/// This trait enables dynamic dispatch over different Arc types (e.g., `Arc<String>`,
/// `Arc<Vec<u8>>`) by erasing the inner type. It is automatically implemented for
/// any type that implements [`ArcErase`].
///
/// Primary use cases:
/// - Storing heterogeneous Arcs in a single collection
/// - Serializing Arcs when the concrete type is not statically known
pub trait ArcEraseDyn: std::any::Any + Send + Sync + 'static {
    // we could use upcasting for this, but we hold Box<dyn ArcEraseDyn> and the box itself also implements Any and so it's easy to accidentally upcast the wrong thing.
    fn as_arc_any(&self) -> &dyn std::any::Any;

    fn type_name(&self) -> &'static str;

    fn identity(&self) -> usize;

    fn clone_dyn(&self) -> Box<dyn ArcEraseDyn>;

    fn downgrade(&self) -> Box<dyn WeakEraseDyn>;

    /// Associates this arc with a storage key.
    ///
    /// For pagable arcs, this marks the arc as paged out and associates it with
    /// the given key for future retrieval. For non-pagable arcs (e.g., `std::sync::Arc`),
    /// this is a no-op.
    fn set_data_key(&self, k: DataKey);

    /// Returns the storage key already associated with this arc, if any.
    fn data_key(&self) -> Option<DataKey>;

    /// Returns true if this arc needs to be written to storage.
    ///
    /// For pagable arcs, returns true if the arc doesn't have a storage key yet.
    /// For non-pagable arcs, always returns false.
    fn needs_paging_out(&self) -> bool;

    /// Serializes this arc using the paging serializer.
    ///
    /// This is used during recursive serialization to convert arcs to their serialized form.
    fn serialize(&self, ser: &mut dyn PagableSerializer) -> crate::Result<ArcSerializeOutcome>;
}
static_assertions::assert_obj_safe!(ArcEraseDyn);

/// Convenience helper for implementing [`PagableDeserialize`] on [`ArcErase`] types.
///
/// This handles the boilerplate of constructing a type-erased deserialization function
/// and downcasting the result. Use this in `PagableDeserialize` impls for custom Arc types.
pub fn deserialize_arc<'de, T: ArcErase, D: PagableDeserializer<'de> + ?Sized>(
    deserializer: &mut D,
) -> anyhow::Result<T> {
    fn deserialize_fn<A: ArcErase>(
        d: &mut dyn PagableDeserializer<'_>,
        _recipe: Arc<dyn PagableDeserializerRecipe>,
    ) -> crate::Result<Box<dyn ArcEraseDyn>> {
        let arc = A::deserialize_inner(d)?;
        Ok(Box::new(arc))
    }

    let arc_box = deserializer.deserialize_arc(TypeId::of::<T>(), deserialize_fn::<T>)?;
    let arc = arc_box
        .as_arc_any()
        .downcast_ref::<T>()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Type mismatch on deserialize_arc: expected {}, got {}",
                std::any::type_name::<T>(),
                arc_box.type_name()
            )
        })?
        .dupe_strong();
    Ok(arc)
}

impl<T: ArcErase> ArcEraseDyn for T {
    fn clone_dyn(&self) -> Box<dyn ArcEraseDyn> {
        Box::new(T::dupe_strong(self))
    }

    fn as_arc_any(&self) -> &dyn std::any::Any {
        self
    }

    fn type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }

    fn identity(&self) -> usize {
        ArcErase::identity(self)
    }

    fn downgrade(&self) -> Box<dyn WeakEraseDyn> {
        match ArcErase::downgrade(self) {
            Some(weak) => Box::new(weak),
            None => Box::new(StrongAsWeak::new(self.dupe_strong())),
        }
    }

    fn set_data_key(&self, k: DataKey) {
        ArcErase::set_data_key(self, k)
    }

    fn data_key(&self) -> Option<DataKey> {
        ArcErase::data_key(self)
    }

    fn needs_paging_out(&self) -> bool {
        ArcErase::needs_paging_out(self)
    }

    fn serialize(&self, ser: &mut dyn PagableSerializer) -> crate::Result<ArcSerializeOutcome> {
        ArcErase::serialize_inner(self, ser)
    }
}

/// Object-safe trait for type-erased weak reference handling.
///
/// This is the weak reference counterpart to [`ArcEraseDyn`], enabling
/// dynamic dispatch over weak references to different types.
pub trait WeakEraseDyn: std::any::Any + Send + Sync + 'static {
    fn as_weak_any(&self) -> &dyn std::any::Any;
    fn upgrade(&self) -> Option<Box<dyn ArcEraseDyn>>;
    fn is_expired(&self) -> bool;
}
static_assertions::assert_obj_safe!(WeakEraseDyn);
impl<T: WeakErase> WeakEraseDyn for T {
    fn as_weak_any(&self) -> &dyn std::any::Any {
        self
    }
    fn upgrade(&self) -> Option<Box<dyn ArcEraseDyn>> {
        WeakErase::upgrade_weak(self)
    }
    fn is_expired(&self) -> bool {
        WeakErase::is_expired(self)
    }
}

/// Trait for weak reference types that can be type-erased.
///
/// Implemented for weak reference types like `std::sync::Weak<T>`.
/// Types implementing this trait can be stored as `Box<dyn WeakEraseDyn>`.
pub trait WeakErase: std::any::Any + Sized + Send + Sync + 'static {
    /// Returns true once no strong reference to the allocation remains, not
    /// counting any this value itself holds (see [`StrongAsWeak`]).
    fn is_expired(&self) -> bool;
    /// Attempts to upgrade to a strong reference, returning None if expired.
    fn upgrade_weak(&self) -> Option<Box<dyn ArcEraseDyn>>;
}

/// A strong reference standing in for a weak one: what [`ArcEraseDyn::downgrade`]
/// hands out for an Arc type whose [`ArcErase::downgrade`] returns `None`.
///
/// Because it keeps the allocation alive, it reports expiry through
/// [`ArcErase::is_unique`] instead: once it is the last reference, nothing else
/// can observe the allocation, so a cache holding it may drop it.
pub struct StrongAsWeak<A>(A);

impl<A: ArcErase> StrongAsWeak<A> {
    pub fn new(arc: A) -> Self {
        Self(arc)
    }
}

impl<A: ArcErase> WeakErase for StrongAsWeak<A> {
    fn is_expired(&self) -> bool {
        self.0.is_unique()
    }

    fn upgrade_weak(&self) -> Option<Box<dyn ArcEraseDyn>> {
        Some(Box::new(self.0.dupe_strong()))
    }
}

/// Trait for Arc types that support pagable serialization.
///
/// This trait is the core abstraction for serializing Arc types while preserving
/// their identity. It provides methods for:
/// - Cloning the Arc (`dupe_strong`)
/// - Getting a unique identity for deduplication (`identity`)
/// - Serializing/deserializing the inner value (`serialize_inner`/`deserialize_inner`)
/// - Downgrading to a weak reference (`downgrade`), when the type has one
///
/// # Implementations
///
/// The pagable crate provides implementations for:
/// - `std::sync::Arc<T>`
/// - `std::sync::Arc<[T]>`
/// - `triomphe::Arc<T>`
/// - `triomphe::Arc<[T]>`
/// - `triomphe::ThinArc<H, T>`
///
/// where `T` (and `H`) implement `PagableSerialize + PagableDeserialize`.
pub trait ArcErase: std::any::Any + Sized + Send + Sync + 'static {
    type Weak: WeakErase + 'static;

    fn dupe_strong(&self) -> Self;
    /// A weak reference to the allocation, or `None` for a type that has no
    /// weak form. Every Arc still gets one through the erased view: for `None`,
    /// [`ArcEraseDyn::downgrade`] holds a [`StrongAsWeak`], which expires per
    /// [`is_unique`](Self::is_unique).
    fn downgrade(&self) -> Option<Self::Weak>;
    fn erase_type() -> impl ArcEraseType;
    fn identity(&self) -> usize;

    /// Returns true when `self` is the only strong reference to its allocation.
    ///
    /// Consulted only through [`StrongAsWeak`], so it matters only for Arc
    /// types without a weak form. The default never reports uniqueness, which
    /// keeps such an Arc cached for the life of the cache; that is right for
    /// immortal allocations such as interned values.
    fn is_unique(&self) -> bool {
        false
    }

    /// Associates this arc with a storage key.
    ///
    /// For pagable arcs, this marks the arc as paged out and associates it with
    /// the given key for future retrieval. For non-pagable arcs (e.g., `std::sync::Arc`),
    /// this is a no-op.
    fn set_data_key(&self, _k: DataKey) {
        // no-op
    }

    /// Returns the storage key already associated with this arc, if any.
    fn data_key(&self) -> Option<DataKey> {
        None
    }

    /// Returns true if this arc needs to be written to storage.
    ///
    /// For pagable arcs, returns true if the arc doesn't have a storage key yet.
    /// For non-pagable arcs, always returns false.
    fn needs_paging_out(&self) -> bool {
        false
    }

    fn serialize_inner(
        &self,
        ser: &mut dyn PagableSerializer,
    ) -> crate::Result<ArcSerializeOutcome>;

    fn deserialize_inner<'de, D: PagableDeserializer<'de> + ?Sized>(
        deser: &mut D,
    ) -> crate::Result<Self>;
}

/// The weak form of an Arc type that has none: never handed out, since
/// [`ArcErase::downgrade`] returns `None` for such a type.
impl WeakErase for () {
    fn is_expired(&self) -> bool {
        true
    }

    fn upgrade_weak(&self) -> Option<Box<dyn ArcEraseDyn>> {
        None
    }
}

impl<T: ?Sized + PagableSerialize + for<'de> PagableBoxDeserialize<'de> + Send + Sync + 'static>
    WeakErase for std::sync::Weak<T>
{
    fn is_expired(&self) -> bool {
        self.strong_count() == 0
    }

    fn upgrade_weak(&self) -> Option<Box<dyn ArcEraseDyn>> {
        self.upgrade().map(|t| Box::new(t) as _)
    }
}

impl<T: ?Sized + PagableSerialize + for<'de> PagableBoxDeserialize<'de> + Send + Sync + 'static>
    ArcErase for std::sync::Arc<T>
{
    type Weak = std::sync::Weak<T>;
    fn dupe_strong(&self) -> Self {
        self.dupe()
    }

    fn erase_type() -> impl ArcEraseType {
        StdArcEraseType::<Self>::new()
    }

    fn identity(&self) -> usize {
        Arc::as_ptr(self) as *const () as usize
    }

    fn downgrade(&self) -> Option<Self::Weak> {
        Some(Arc::downgrade(self))
    }

    fn serialize_inner(
        &self,
        ser: &mut dyn PagableSerializer,
    ) -> crate::Result<ArcSerializeOutcome> {
        T::pagable_serialize_arc_payload(self.dupe(), ser)?;
        Ok(ArcSerializeOutcome::Serialized)
    }

    fn deserialize_inner<'de, D: PagableDeserializer<'de> + ?Sized>(
        deser: &mut D,
    ) -> crate::Result<Self> {
        T::deserialize_arc_payload(deser)
    }
}

impl<T: PagableSerialize + for<'de> PagableDeserialize<'de> + Send + Sync + 'static> ArcErase
    for triomphe::Arc<T>
{
    type Weak = ();
    fn dupe_strong(&self) -> Self {
        (*self).clone()
    }

    fn erase_type() -> impl ArcEraseType {
        StdArcEraseType::<Self>::new()
    }

    fn identity(&self) -> usize {
        self.as_ptr() as usize
    }

    fn downgrade(&self) -> Option<Self::Weak> {
        None
    }

    fn is_unique(&self) -> bool {
        triomphe::Arc::is_unique(self)
    }

    fn serialize_inner(
        &self,
        ser: &mut dyn PagableSerializer,
    ) -> crate::Result<ArcSerializeOutcome> {
        T::pagable_serialize(self, ser)?;
        Ok(ArcSerializeOutcome::Serialized)
    }

    fn deserialize_inner<'de, D: PagableDeserializer<'de> + ?Sized>(
        deser: &mut D,
    ) -> crate::Result<Self> {
        Ok(Self::new(T::pagable_deserialize(deser)?))
    }
}

impl ArcErase for triomphe::Arc<str> {
    type Weak = ();
    fn dupe_strong(&self) -> Self {
        (*self).clone()
    }

    fn erase_type() -> impl ArcEraseType {
        StdArcEraseType::<Self>::new()
    }

    fn identity(&self) -> usize {
        self.as_ptr() as *const () as usize
    }

    fn downgrade(&self) -> Option<Self::Weak> {
        None
    }

    fn is_unique(&self) -> bool {
        triomphe::Arc::is_unique(self)
    }

    fn serialize_inner(
        &self,
        ser: &mut dyn PagableSerializer,
    ) -> crate::Result<ArcSerializeOutcome> {
        <str as PagableSerialize>::pagable_serialize(self, ser)?;
        Ok(ArcSerializeOutcome::Serialized)
    }

    fn deserialize_inner<'de, D: PagableDeserializer<'de> + ?Sized>(
        deser: &mut D,
    ) -> crate::Result<Self> {
        let s: String = String::pagable_deserialize(deser)?;
        Ok(s.into())
    }
}

impl<T: PagableSerialize + for<'de> PagableDeserialize<'de> + Send + Sync + 'static> ArcErase
    for triomphe::Arc<[T]>
{
    type Weak = ();
    fn dupe_strong(&self) -> Self {
        (*self).clone()
    }

    fn erase_type() -> impl ArcEraseType {
        StdArcEraseType::<Self>::new()
    }

    fn identity(&self) -> usize {
        self.as_ptr() as *const T as usize
    }

    fn downgrade(&self) -> Option<Self::Weak> {
        None
    }

    fn is_unique(&self) -> bool {
        triomphe::Arc::is_unique(self)
    }

    fn serialize_inner(
        &self,
        ser: &mut dyn PagableSerializer,
    ) -> crate::Result<ArcSerializeOutcome> {
        <[T] as PagableSerialize>::pagable_serialize(self, ser)?;
        Ok(ArcSerializeOutcome::Serialized)
    }

    fn deserialize_inner<'de, D: PagableDeserializer<'de> + ?Sized>(
        deser: &mut D,
    ) -> crate::Result<Self> {
        let vec: Vec<_> = <Box<[T]>>::pagable_deserialize(deser)?.into();
        Ok(vec.into())
    }
}

impl<
    H: PagableSerialize + for<'de> PagableDeserialize<'de> + Send + Sync + 'static,
    T: PagableSerialize + for<'de> PagableDeserialize<'de> + Send + Sync + 'static,
> ArcErase for triomphe::ThinArc<H, T>
{
    type Weak = ();
    fn dupe_strong(&self) -> Self {
        (*self).clone()
    }

    fn erase_type() -> impl ArcEraseType {
        StdArcEraseType::<Self>::new()
    }

    fn identity(&self) -> usize {
        self.as_ptr() as usize
    }

    fn downgrade(&self) -> Option<Self::Weak> {
        None
    }

    fn is_unique(&self) -> bool {
        triomphe::ThinArc::strong_count(self) == 1
    }

    fn serialize_inner(
        &self,
        ser: &mut dyn PagableSerializer,
    ) -> crate::Result<ArcSerializeOutcome> {
        self.header.header.pagable_serialize(ser)?;
        self.slice.pagable_serialize(ser)?;
        Ok(ArcSerializeOutcome::Serialized)
    }

    fn deserialize_inner<'de, D: PagableDeserializer<'de> + ?Sized>(
        deser: &mut D,
    ) -> crate::Result<Self> {
        let header = H::pagable_deserialize(deser)?;
        let slice: Box<[T]> = <Box<[T]>>::pagable_deserialize(deser)?;
        Ok(Self::from_header_and_iter(
            header,
            slice.into_vec().into_iter(),
        ))
    }
}

/// Trait for deserializing type-erased Arcs.
///
/// This trait enables deserializing an Arc when only the type metadata is known
/// at runtime, not the concrete Rust type. Used for dynamic deserialization
/// scenarios where the Arc type is determined by stored metadata.
#[async_trait::async_trait(?Send)]
pub trait ArcEraseType: Send + Sync + 'static {
    fn type_name(&self) -> &'static str;
}
static_assertions::assert_obj_safe!(ArcEraseType);

#[async_trait::async_trait(?Send)]
impl<W: 'static, A: ArcErase<Weak = W>> ArcEraseType for StdArcEraseType<A> {
    fn type_name(&self) -> &'static str {
        std::any::type_name::<A>()
    }
}

/// Helper type for implementing [`ArcEraseType`] for standard Arc types.
///
/// This is a zero-sized type that carries the Arc type information and
/// implements `ArcEraseType` for any `ArcErase` implementor.
pub struct StdArcEraseType<T: 'static>(std::marker::PhantomData<T>);

impl<T: 'static> Default for StdArcEraseType<T> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<T: 'static> StdArcEraseType<T> {
    pub fn new() -> Self {
        Self::default()
    }
}

unsafe impl<T: 'static> Send for StdArcEraseType<T> {}
unsafe impl<T: 'static> Sync for StdArcEraseType<T> {}

impl<W: 'static, A: ArcErase<Weak = W>> StdArcEraseType<A> {}
