/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![allow(unused)]

use std::any::TypeId;

use dupe::Dupe;
use strong_hash::StrongHash;

use crate::PagableBoxDeserialize;
use crate::PagableDeserialize;
use crate::PagableDeserializer;
use crate::PagableSerialize;
use crate::PagableSerializer;
use crate::arc_erase::ArcErase;
use crate::arc_erase::ArcEraseDyn;
use crate::arc_erase::deserialize_arc;

impl<
    T: ?Sized
        + for<'a> PagableBoxDeserialize<'a>
        + PagableSerialize
        + Send
        + Sync
        + std::any::Any
        + 'static,
> PagableSerialize for std::sync::Arc<T>
{
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        serializer.serialize_arc(self)
    }
}

impl<
    'de,
    T: ?Sized
        + for<'a> PagableBoxDeserialize<'a>
        + PagableSerialize
        + Send
        + Sync
        + std::any::Any
        + 'static,
> PagableDeserialize<'de> for std::sync::Arc<T>
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        deserialize_arc::<Self, _>(deserializer)
    }
}

impl<T: for<'a> PagableDeserialize<'a> + PagableSerialize + Send + Sync + std::any::Any + 'static>
    PagableSerialize for triomphe::Arc<T>
{
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        serializer.serialize_arc(self)
    }
}

impl<
    'de,
    T: for<'a> PagableDeserialize<'a> + PagableSerialize + Send + Sync + std::any::Any + 'static,
> PagableDeserialize<'de> for triomphe::Arc<T>
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        deserialize_arc::<Self, _>(deserializer)
    }
}

impl<T: for<'a> PagableDeserialize<'a> + PagableSerialize + Send + Sync + std::any::Any + 'static>
    PagableSerialize for triomphe::Arc<[T]>
{
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        serializer.serialize_arc(self)
    }
}

impl<
    'de,
    T: for<'a> PagableDeserialize<'a> + PagableSerialize + Send + Sync + std::any::Any + 'static,
> PagableDeserialize<'de> for triomphe::Arc<[T]>
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        deserialize_arc::<Self, _>(deserializer)
    }
}

impl PagableSerialize for triomphe::Arc<str> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        serializer.serialize_arc(self)
    }
}

impl<'de> PagableDeserialize<'de> for triomphe::Arc<str> {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        deserialize_arc::<Self, _>(deserializer)
    }
}

impl<
    H: for<'a> PagableDeserialize<'a> + PagableSerialize + Send + Sync + std::any::Any,
    T: for<'a> PagableDeserialize<'a> + PagableSerialize + Send + Sync + std::any::Any,
> PagableSerialize for triomphe::ThinArc<H, T>
{
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        serializer.serialize_arc(self)
    }
}

impl<
    'de,
    H: PagableSerialize + for<'a> PagableDeserialize<'a> + Send + Sync + std::any::Any,
    T: PagableSerialize + for<'a> PagableDeserialize<'a> + Send + Sync + std::any::Any,
> PagableDeserialize<'de> for triomphe::ThinArc<H, T>
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        deserialize_arc::<Self, _>(deserializer)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::testing::TestingDeserializer;
    use crate::testing::TestingSerializer;
    use crate::traits::PagableDeserialize;
    use crate::traits::PagableSerialize;

    #[test]
    fn test_arc_str_roundtrip() -> crate::Result<()> {
        let t1: Arc<str> = Arc::from("hello world");
        let mut serializer = TestingSerializer::new();
        t1.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();
        let mut deserializer = TestingDeserializer::new(&bytes);
        let t2: Arc<str> = Arc::pagable_deserialize(&mut deserializer)?;
        assert_eq!(t1, t2);
        Ok(())
    }
}
