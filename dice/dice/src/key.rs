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
use std::any::TypeId;
use std::fmt::Display;
use std::fmt::Formatter;
use std::hash::Hash;
use std::hash::Hasher;
use std::mem;
use std::sync::Arc as StdArc;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_hash::BuckHasher;
use cmp_any::PartialEqAny;
use derive_more::Display;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use pagable::PagableDeserializer;
use pagable::PagableSerializer;
#[cfg(feature = "pagable")]
use pagable::PagableTagged;
#[cfg(feature = "pagable")]
use pagable::pagable_typetag;

#[cfg(not(feature = "pagable"))]
pub trait PagableTagged {}
#[cfg(not(feature = "pagable"))]
impl<T> PagableTagged for T {}

use crate::Demand;
use crate::api::computations::DiceComputations;
use crate::api::demand::DemandRef;
use crate::api::demand::DemandValue;
use crate::api::key::Key;
use crate::api::key::ValueSerialize;
use crate::api::projection::DiceProjectionComputations;
use crate::api::projection::ProjectionKey;
use crate::api::storage_type::StorageType;
use crate::value::DiceKeyValue;
use crate::value::DiceProjectValue;
use crate::value::DiceValueDyn;
use crate::value::MaybeValidDiceValue;

/// Type erased internal dice key
/// A key as the core state names it: an index into the `DiceKeyIndex`.
pub(crate) type DiceKey = dice_core::Key;

/// Key of the parent computation
#[derive(
    Allocative, Eq, PartialEq, Clone, Copy, Dupe, Hash, Debug, Ord, PartialOrd
)]
pub(crate) enum ParentKey {
    None,
    Some(DiceKey),
}

#[derive(Allocative, Clone, Dupe, Display)]
pub(crate) enum DiceKeyErased {
    Key(StdArc<dyn DiceKeyDyn>),
    Projection(ProjectionWithBase),
}

impl DiceKeyErased {
    pub(crate) fn key<K: Key>(k: K) -> Self {
        Self::Key(StdArc::new(k))
    }

    #[allow(unused)] // counterpart of projection to `key`
    pub(crate) fn proj<K: ProjectionKey>(base: DiceKey, k: K) -> Self {
        Self::Projection(ProjectionWithBase {
            base,
            proj: StdArc::new(k),
        })
    }

    pub(crate) fn key_type_name(&self) -> &'static str {
        match self {
            DiceKeyErased::Key(k) => k.key_type_name(),
            DiceKeyErased::Projection(k) => k.proj.key_type_name(),
        }
    }

    pub(crate) fn equality_is_always_unequal(&self) -> bool {
        match self {
            DiceKeyErased::Key(k) => k.equality_is_always_unequal(),
            DiceKeyErased::Projection(k) => k.proj.equality_is_always_unequal(),
        }
    }

    pub(crate) fn hash(&self) -> u64 {
        match self {
            DiceKeyErased::Key(k) => k.hash(),
            DiceKeyErased::Projection(proj) => proj.hash(),
        }
    }

    pub(crate) fn as_any(&self) -> &dyn Any {
        match self {
            DiceKeyErased::Key(k) => k.as_any(),
            DiceKeyErased::Projection(proj) => proj.proj().as_any(),
        }
    }

    pub(crate) fn request_value<T: 'static>(&self) -> Option<T> {
        let mut demand_impl = DemandValue { value: None };
        let mut demand = Demand::new(&mut demand_impl);
        match self {
            DiceKeyErased::Key(k) => k.provide(&mut demand),
            DiceKeyErased::Projection(..) => {}
        }
        demand_impl.value
    }

    pub(crate) fn request_ref<T: ?Sized + 'static>(&self) -> Option<&T> {
        let mut demand_impl = DemandRef { value: None };
        let mut demand = Demand::new(&mut demand_impl);
        match self {
            DiceKeyErased::Key(k) => k.provide(&mut demand),
            DiceKeyErased::Projection(..) => {}
        }
        demand_impl.value
    }

    pub(crate) fn downcast<K: 'static>(self) -> Option<StdArc<K>> {
        match self {
            DiceKeyErased::Key(k) => {
                if k.as_any().is::<K>() {
                    Some(unsafe { StdArc::from_raw(StdArc::into_raw(k).cast()) })
                } else {
                    None
                }
            }
            DiceKeyErased::Projection(proj) => {
                if proj.proj.as_any().is::<K>() {
                    Some(unsafe { StdArc::from_raw(StdArc::into_raw(proj.proj).cast()) })
                } else {
                    None
                }
            }
        }
    }

    pub(crate) fn as_ref(&self) -> DiceKeyErasedRef<'_> {
        match self {
            DiceKeyErased::Key(k) => DiceKeyErasedRef::Key(&**k),
            DiceKeyErased::Projection(proj) => {
                DiceKeyErasedRef::Projection(ProjectionWithBaseRef {
                    base: proj.base,
                    proj: &*proj.proj,
                })
            }
        }
    }
}

impl PartialEq for DiceKeyErased {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (DiceKeyErased::Key(k), DiceKeyErased::Key(ok)) => k.cmp_any() == ok.cmp_any(),
            (DiceKeyErased::Projection(k), DiceKeyErased::Projection(ok)) => k == ok,
            _ => false,
        }
    }
}

impl Eq for DiceKeyErased {}

#[derive(Copy, Clone, Dupe, Display)]
pub(crate) enum DiceKeyErasedRef<'a> {
    Key(&'a dyn DiceKeyDyn),
    Projection(ProjectionWithBaseRef<'a>),
}

impl<'a> DiceKeyErasedRef<'a> {
    pub(crate) fn key<K: Key>(k: &'a K) -> Self {
        Self::Key(k)
    }

    pub(crate) fn proj<K: ProjectionKey>(base: DiceKey, k: &'a K) -> Self {
        Self::Projection(ProjectionWithBaseRef { base, proj: k })
    }

    pub(crate) fn hash(&self) -> u64 {
        match self {
            DiceKeyErasedRef::Key(k) => k.hash(),
            DiceKeyErasedRef::Projection(p) => p.hash(),
        }
    }

    #[allow(unused)] // generally useful function that is the counterpart on `DiceKeyErased`
    pub(crate) fn key_type_name(&self) -> &'static str {
        match self {
            DiceKeyErasedRef::Key(k) => k.key_type_name(),
            DiceKeyErasedRef::Projection(k) => k.proj.key_type_name(),
        }
    }

    fn to_owned(self) -> DiceKeyErased {
        match self {
            DiceKeyErasedRef::Key(k) => DiceKeyErased::Key(k.clone_arc()),
            DiceKeyErasedRef::Projection(p) => DiceKeyErased::Projection(p.to_owned()),
        }
    }
}

impl PartialEq for DiceKeyErasedRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (DiceKeyErasedRef::Key(k), DiceKeyErasedRef::Key(ok)) => k.cmp_any() == ok.cmp_any(),
            (DiceKeyErasedRef::Projection(k), DiceKeyErasedRef::Projection(ok)) => k == ok,
            _ => false,
        }
    }
}

#[derive(Display)]
pub(crate) enum CowDiceKey<'a> {
    Ref(DiceKeyErasedRef<'a>),
    Owned(DiceKeyErased),
}

impl<'a> CowDiceKey<'a> {
    pub(crate) fn into_owned(self) -> DiceKeyErased {
        match self {
            CowDiceKey::Ref(r) => r.to_owned(),
            CowDiceKey::Owned(owned) => owned,
        }
    }

    pub(crate) fn borrow(&'a self) -> DiceKeyErasedRef<'a> {
        match self {
            CowDiceKey::Ref(r) => *r,
            CowDiceKey::Owned(owned) => owned.as_ref(),
        }
    }
}

fn key_hash<K: Hash + 'static>(key: &K) -> u64 {
    let mut hasher = BuckHasher::default();
    if mem::size_of::<K>() == 0 {
        // Hashing `TypeId` unconditionally measurably slows down hashing.
        TypeId::of::<K>().hash(&mut hasher);
    } else {
        key.hash(&mut hasher);
    }
    hasher.finish()
}

pub(crate) struct CowDiceKeyHashed<'a> {
    cow: CowDiceKey<'a>,
    hash: u64,
}

impl<'a> CowDiceKeyHashed<'a> {
    pub(crate) fn key_ref<K: Key>(key: &'a K) -> CowDiceKeyHashed<'a> {
        let hash = key_hash(key);
        CowDiceKeyHashed {
            cow: CowDiceKey::Ref(DiceKeyErasedRef::key(key)),
            hash,
        }
    }

    pub(crate) fn proj_ref<K: ProjectionKey>(base: DiceKey, k: &'a K) -> CowDiceKeyHashed<'a> {
        let key = DiceKeyErasedRef::proj(base, k);
        let hash = key.hash();
        CowDiceKeyHashed {
            cow: CowDiceKey::Ref(key),
            hash,
        }
    }

    pub(crate) fn key<K: Key>(key: K) -> CowDiceKeyHashed<'static> {
        let hash = key_hash(&key);
        CowDiceKeyHashed {
            cow: CowDiceKey::Owned(DiceKeyErased::key(key)),
            hash,
        }
    }

    pub(crate) fn hash(&self) -> u64 {
        self.hash
    }

    pub(crate) fn into_cow(self) -> CowDiceKey<'a> {
        self.cow
    }
}

impl Display for CowDiceKeyHashed<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.cow)
    }
}

#[cfg_attr(feature = "pagable", pagable_typetag)]
#[async_trait]
pub trait DiceKeyDyn: Allocative + Display + Send + Sync + PagableTagged + 'static {
    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        cancellations: &CancellationContext,
    ) -> StdArc<dyn DiceValueDyn>;

    fn cmp_any(&self) -> PartialEqAny<'_>;

    fn hash(&self) -> u64;

    fn as_any(&self) -> &dyn Any;

    fn clone_arc(&self) -> StdArc<dyn DiceKeyDyn>;

    fn key_type_name(&self) -> &'static str;

    fn storage_type(&self) -> StorageType;

    fn equality_is_always_unequal(&self) -> bool;

    fn provide<'a>(&'a self, demand: &mut Demand<'a>);

    /// Serializes a value associated with this key via the key's `ValueSerialize`.
    fn pagable_serialize_value(
        &self,
        value: &dyn DiceValueDyn,
        ser: &mut dyn PagableSerializer,
    ) -> Option<pagable::Result<()>>;

    /// Deserializes a value associated with this key via the key's `ValueSerialize`.
    fn pagable_deserialize_value<'de>(
        &self,
        deser: &mut dyn PagableDeserializer<'de>,
    ) -> pagable::Result<StdArc<dyn DiceValueDyn>>;
}

#[async_trait]
impl<K> DiceKeyDyn for K
where
    K: Key,
{
    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        cancellations: &CancellationContext,
    ) -> StdArc<dyn DiceValueDyn> {
        let value = self.compute(ctx, cancellations).await;
        StdArc::new(DiceKeyValue::<K>::new(value))
    }

    fn cmp_any(&self) -> PartialEqAny<'_> {
        PartialEqAny::new(self)
    }

    fn hash(&self) -> u64 {
        key_hash(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_arc(&self) -> StdArc<dyn DiceKeyDyn> {
        StdArc::new(self.clone())
    }

    fn key_type_name(&self) -> &'static str {
        K::key_type_name()
    }

    fn storage_type(&self) -> StorageType {
        K::storage_type()
    }

    fn equality_is_always_unequal(&self) -> bool {
        K::equality_behavior().is_always_unequal()
    }

    fn provide<'a>(&'a self, demand: &mut Demand<'a>) {
        K::provide(self, demand)
    }

    fn pagable_serialize_value(
        &self,
        value: &dyn DiceValueDyn,
        ser: &mut dyn PagableSerializer,
    ) -> Option<pagable::Result<()>> {
        let typed_value: &K::Value = value
            .downcast_ref()
            .expect("type mismatch between Key and value during pagable serialization");
        K::value_serialize().pagable_serialize_value(typed_value, ser)
    }

    fn pagable_deserialize_value<'de>(
        &self,
        deser: &mut dyn PagableDeserializer<'de>,
    ) -> pagable::Result<StdArc<dyn DiceValueDyn>> {
        let typed_value: K::Value = K::value_serialize().pagable_deserialize_value(deser)?;
        Ok(StdArc::new(DiceKeyValue::<K>::new(typed_value)))
    }
}

#[cfg_attr(feature = "pagable", pagable_typetag)]
pub trait DiceProjectionDyn: Allocative + Display + Send + Sync + PagableTagged + 'static {
    fn compute(
        &self,
        derive_from: &MaybeValidDiceValue,
        ctx: &DiceProjectionComputations,
    ) -> StdArc<dyn DiceValueDyn>;

    fn cmp_any(&self) -> PartialEqAny<'_>;

    fn hash(&self) -> u64;

    fn as_any(&self) -> &dyn Any;

    fn clone_arc(&self) -> StdArc<dyn DiceProjectionDyn>;

    fn key_type_name(&self) -> &'static str;

    fn storage_type(&self) -> StorageType;

    fn equality_is_always_unequal(&self) -> bool;

    /// See [`DiceKeyDyn::pagable_serialize_value`].
    fn pagable_serialize_value(
        &self,
        value: &dyn DiceValueDyn,
        ser: &mut dyn PagableSerializer,
    ) -> Option<pagable::Result<()>>;

    /// See [`DiceKeyDyn::pagable_deserialize_value`].
    fn pagable_deserialize_value<'de>(
        &self,
        deser: &mut dyn PagableDeserializer<'de>,
    ) -> pagable::Result<StdArc<dyn DiceValueDyn>>;
}

impl<K> DiceProjectionDyn for K
where
    K: ProjectionKey,
{
    fn compute(
        &self,
        derive_from: &MaybeValidDiceValue,
        ctx: &DiceProjectionComputations,
    ) -> StdArc<dyn DiceValueDyn> {
        let value = self.compute(
            derive_from
                .downcast_maybe_transient()
                .expect("Projection Key derived from wrong type"),
            ctx,
        );
        StdArc::new(DiceProjectValue::<K>::new(value))
    }

    fn cmp_any(&self) -> PartialEqAny<'_> {
        PartialEqAny::new(self)
    }

    fn hash(&self) -> u64 {
        key_hash(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_arc(&self) -> StdArc<dyn DiceProjectionDyn> {
        StdArc::new(self.clone())
    }

    fn key_type_name(&self) -> &'static str {
        K::key_type_name()
    }

    fn storage_type(&self) -> StorageType {
        K::storage_type()
    }

    fn equality_is_always_unequal(&self) -> bool {
        K::equality_behavior().is_always_unequal()
    }

    fn pagable_serialize_value(
        &self,
        value: &dyn DiceValueDyn,
        ser: &mut dyn PagableSerializer,
    ) -> Option<pagable::Result<()>> {
        let typed_value: &K::Value = value
            .downcast_ref()
            .expect("type mismatch between ProjectionKey and value during pagable serialization");
        K::value_serialize().pagable_serialize_value(typed_value, ser)
    }

    fn pagable_deserialize_value<'de>(
        &self,
        deser: &mut dyn PagableDeserializer<'de>,
    ) -> pagable::Result<StdArc<dyn DiceValueDyn>> {
        let typed_value: K::Value = K::value_serialize().pagable_deserialize_value(deser)?;
        Ok(StdArc::new(DiceProjectValue::<K>::new(typed_value)))
    }
}

#[derive(Allocative, Clone, Dupe)]
pub(crate) struct ProjectionWithBase {
    base: DiceKey,
    proj: StdArc<dyn DiceProjectionDyn>,
}

impl ProjectionWithBase {
    pub(crate) fn base(&self) -> DiceKey {
        self.base
    }

    pub(crate) fn proj(&self) -> &dyn DiceProjectionDyn {
        &*self.proj
    }

    fn hash(&self) -> u64 {
        let mut hasher = BuckHasher::default();

        self.base.hash(&mut hasher);
        self.proj.hash().hash(&mut hasher);

        hasher.finish()
    }
}

impl PartialEq for ProjectionWithBase {
    fn eq(&self, other: &Self) -> bool {
        self.proj.cmp_any() == other.proj.cmp_any() && self.base == other.base
    }
}

impl Eq for ProjectionWithBase {}

impl Display for ProjectionWithBase {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.proj)
    }
}

#[derive(Copy, Clone, Dupe)]
pub(crate) struct ProjectionWithBaseRef<'a> {
    base: DiceKey,
    proj: &'a dyn DiceProjectionDyn,
}

impl ProjectionWithBaseRef<'_> {
    fn to_owned(self) -> ProjectionWithBase {
        ProjectionWithBase {
            base: self.base,
            proj: self.proj.clone_arc(),
        }
    }

    fn hash(&self) -> u64 {
        let mut hasher = BuckHasher::default();

        self.base.hash(&mut hasher);
        self.proj.hash().hash(&mut hasher);

        hasher.finish()
    }
}

impl PartialEq for ProjectionWithBaseRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.proj.cmp_any() == other.proj.cmp_any() && self.base == other.base
    }
}

impl Eq for ProjectionWithBaseRef<'_> {}

impl Display for ProjectionWithBaseRef<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.proj)
    }
}

mod introspection {
    use std::fmt::Display;
    use std::fmt::Formatter;
    use std::hash::Hash;
    use std::hash::Hasher;

    use cmp_any::PartialEqAny;
    use dupe::Dupe;

    use crate::introspection::graph::AnyKey;
    use crate::introspection::graph::KeyForIntrospection;
    use crate::key::DiceKeyErased;

    impl DiceKeyErased {
        pub(crate) fn introspect(&self) -> AnyKey {
            #[derive(Clone, Dupe)]
            struct Wrap(DiceKeyErased);

            impl PartialEq for Wrap {
                fn eq(&self, other: &Self) -> bool {
                    self.0 == other.0
                }
            }

            impl Eq for Wrap {}

            impl Display for Wrap {
                fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                    match &self.0 {
                        DiceKeyErased::Key(k) => {
                            write!(f, "{k}")
                        }
                        DiceKeyErased::Projection(p) => {
                            write!(f, "{}", p.proj)
                        }
                    }
                }
            }

            impl KeyForIntrospection for Wrap {
                fn get_key_equality(&self) -> PartialEqAny<'_> {
                    PartialEqAny::new(self)
                }

                fn hash(&self, mut state: &mut dyn Hasher) {
                    Hash::hash(&self.0.hash(), &mut state);
                }

                fn box_clone(&self) -> Box<dyn KeyForIntrospection> {
                    Box::new(self.clone())
                }

                fn type_name(&self) -> &'static str {
                    match &self.0 {
                        DiceKeyErased::Key(k) => k.key_type_name(),
                        DiceKeyErased::Projection(p) => p.proj.key_type_name(),
                    }
                }
            }

            AnyKey::new(Wrap(self.dupe()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use allocative::Allocative;
    use derive_more::Display;
    use dice_futures::cancellation::CancellationContext;
    use dupe::Dupe;
    use pagable::Pagable;
    use pagable::pagable_typetag;

    use crate::DiceKeyDyn;
    use crate::api::computations::DiceComputations;
    use crate::api::key::Key;
    use crate::api::key::NoValueSerialize;
    use crate::api::key::ValueSerialize;
    use crate::api::projection::DiceProjectionComputations;
    use crate::api::projection::ProjectionKey;
    use crate::key::DiceKey;
    use crate::key::DiceKeyErased;
    use crate::key::DiceProjectionDyn;

    #[test]
    fn downcast_key_does_not_increase_refs() {
        #[derive(Allocative, Debug, Display, Clone, Dupe, Eq, PartialEq, Hash, Pagable)]
        #[pagable_typetag(DiceKeyDyn)]
        struct TestK;

        #[async_trait::async_trait]
        impl Key for TestK {
            type Value = ();

            async fn compute(
                &self,
                _ctx: &mut DiceComputations,
                _cancellations: &CancellationContext,
            ) -> Self::Value {
                unimplemented!("test")
            }

            fn equality_behavior() -> crate::EqualityBehavior<Self::Value> {
                crate::EqualityBehavior::Compare(|_x, _y| unimplemented!("test"))
            }

            fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
                NoValueSerialize::<Self::Value>::new()
            }
        }

        let erased = DiceKeyErased::key(TestK);

        let downcast = erased.downcast::<TestK>();
        assert!(downcast.is_some());
        let downcast = downcast.unwrap();

        assert_eq!(&*downcast, &TestK);

        // no extra copies
        assert_eq!(Arc::strong_count(&downcast), 1);

        #[derive(Allocative, Debug, Display, Clone, Dupe, Eq, PartialEq, Hash, Pagable)]
        #[pagable_typetag(DiceProjectionDyn)]
        struct TestProj;

        impl ProjectionKey for TestProj {
            type DeriveFromKey = TestK;
            type Value = ();

            fn compute(
                &self,
                _derive_from: &<<Self as ProjectionKey>::DeriveFromKey as Key>::Value,
                _ctx: &DiceProjectionComputations,
            ) -> Self::Value {
                unimplemented!("test")
            }

            fn equality_behavior() -> crate::EqualityBehavior<Self::Value> {
                crate::EqualityBehavior::Compare(|_x, _y| unimplemented!("test"))
            }

            fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
                NoValueSerialize::<Self::Value>::new()
            }
        }

        let erased = DiceKeyErased::proj(DiceKey { index: 99 }, TestProj);

        let downcast = erased.downcast::<TestProj>();
        assert!(downcast.is_some());
        let downcast = downcast.unwrap();

        assert_eq!(&*downcast, &TestProj);

        // no extra copies
        assert_eq!(Arc::strong_count(&downcast), 1);
    }
}
