/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;

use crate::Pagable;
use crate::PagableDeserializer;
use crate::PagableSerializer;

/// Serializes a value to/from pagable storage that may or may not be serializable.
/// Should possibly be called PagableMaybeSerialize.
///
/// `pagable_serialize_value` returns `None` to indicate the value should not be
/// paged out (e.g. error or otherwise non-pagable values), in which case it is
/// kept in memory.
pub trait ValueSerialize {
    type Value: Allocative + Send + Sync + 'static;

    fn pagable_serialize_value(
        &self,
        v: &Self::Value,
        ser: &mut dyn PagableSerializer,
    ) -> Option<crate::Result<()>>;

    fn pagable_deserialize_value<'de, D: PagableDeserializer<'de> + ?Sized>(
        &self,
        deser: &mut D,
    ) -> crate::Result<Self::Value>;
}

/// [`ValueSerialize`] that panics if serialization is attempted, for values whose
/// pagable support is not yet implemented.
pub struct TodoValueSerialize<V>(std::marker::PhantomData<V>);
impl<V> TodoValueSerialize<V> {
    pub fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<V: Allocative + Send + Sync + 'static> ValueSerialize for TodoValueSerialize<V> {
    type Value = V;

    fn pagable_serialize_value(
        &self,
        _v: &Self::Value,
        _ser: &mut dyn PagableSerializer,
    ) -> Option<crate::Result<()>> {
        unimplemented!()
    }

    fn pagable_deserialize_value<'de, D: PagableDeserializer<'de> + ?Sized>(
        &self,
        _deser: &mut D,
    ) -> crate::Result<Self::Value> {
        unimplemented!()
    }
}

/// [`ValueSerialize`] that never pages out, keeping every value in memory.
pub struct NoValueSerialize<V>(std::marker::PhantomData<V>);
impl<V> NoValueSerialize<V> {
    pub fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<V: Allocative + Send + Sync + 'static> ValueSerialize for NoValueSerialize<V> {
    type Value = V;

    fn pagable_serialize_value(
        &self,
        _v: &Self::Value,
        _ser: &mut dyn PagableSerializer,
    ) -> Option<crate::Result<()>> {
        None
    }

    fn pagable_deserialize_value<'de, D: PagableDeserializer<'de> + ?Sized>(
        &self,
        _deser: &mut D,
    ) -> crate::Result<Self::Value> {
        unimplemented!()
    }
}

/// [`ValueSerialize`] for a `Result` value that pages out the `Ok` payload and
/// keeps `Err` values in memory.
pub struct OkPagableValueSerialize<V>(std::marker::PhantomData<V>);

impl<V> OkPagableValueSerialize<V> {
    pub fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<V: Pagable + Allocative + Send + Sync + 'static, E: Allocative + Send + Sync + 'static>
    ValueSerialize for OkPagableValueSerialize<Result<V, E>>
{
    type Value = Result<V, E>;

    fn pagable_serialize_value(
        &self,
        v: &Self::Value,
        ser: &mut dyn PagableSerializer,
    ) -> Option<crate::Result<()>> {
        match v {
            Ok(v) => Some(v.pagable_serialize(ser)),
            Err(_) => None,
        }
    }

    fn pagable_deserialize_value<'de, D: PagableDeserializer<'de> + ?Sized>(
        &self,
        deser: &mut D,
    ) -> crate::Result<Self::Value> {
        Ok(Ok(V::pagable_deserialize(deser)?))
    }
}

/// [`ValueSerialize`] for a [`Pagable`] value that always pages out.
pub struct PagableValueSerialize<V>(std::marker::PhantomData<V>);

impl<V> PagableValueSerialize<V> {
    pub fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<V: Pagable + Allocative + Send + Sync + 'static> ValueSerialize for PagableValueSerialize<V> {
    type Value = V;

    fn pagable_serialize_value(
        &self,
        v: &Self::Value,
        ser: &mut dyn PagableSerializer,
    ) -> Option<crate::Result<()>> {
        Some(v.pagable_serialize(ser))
    }

    fn pagable_deserialize_value<'de, D: PagableDeserializer<'de> + ?Sized>(
        &self,
        deser: &mut D,
    ) -> crate::Result<Self::Value> {
        V::pagable_deserialize(deser)
    }
}
