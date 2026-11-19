/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::traits::PagableDeserialize;
use crate::traits::PagableDeserializer;
use crate::traits::PagableSerialize;
use crate::traits::PagableSerializer;

impl<T1: PagableSerialize, T2: PagableSerialize> PagableSerialize for (T1, T2) {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        let (m1, m2) = self;
        m1.pagable_serialize(serializer)?;
        m2.pagable_serialize(serializer)?;
        Ok(())
    }
}

impl<T1: PagableSerialize, T2: PagableSerialize, T3: PagableSerialize> PagableSerialize
    for (T1, T2, T3)
{
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        let (m1, m2, m3) = self;
        m1.pagable_serialize(serializer)?;
        m2.pagable_serialize(serializer)?;
        m3.pagable_serialize(serializer)?;
        Ok(())
    }
}

impl<T1: PagableSerialize, T2: PagableSerialize, T3: PagableSerialize, T4: PagableSerialize>
    PagableSerialize for (T1, T2, T3, T4)
{
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        let (m1, m2, m3, m4) = self;
        m1.pagable_serialize(serializer)?;
        m2.pagable_serialize(serializer)?;
        m3.pagable_serialize(serializer)?;
        m4.pagable_serialize(serializer)?;
        Ok(())
    }
}

impl<
    T1: PagableSerialize,
    T2: PagableSerialize,
    T3: PagableSerialize,
    T4: PagableSerialize,
    T5: PagableSerialize,
> PagableSerialize for (T1, T2, T3, T4, T5)
{
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        let (m1, m2, m3, m4, m5) = self;
        m1.pagable_serialize(serializer)?;
        m2.pagable_serialize(serializer)?;
        m3.pagable_serialize(serializer)?;
        m4.pagable_serialize(serializer)?;
        m5.pagable_serialize(serializer)?;
        Ok(())
    }
}

impl<'de, T1: PagableDeserialize<'de>, T2: PagableDeserialize<'de>> PagableDeserialize<'de>
    for (T1, T2)
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        let m1 = T1::pagable_deserialize(deserializer)?;
        let m2 = T2::pagable_deserialize(deserializer)?;
        Ok((m1, m2))
    }
}

impl<'de, T1: PagableDeserialize<'de>, T2: PagableDeserialize<'de>, T3: PagableDeserialize<'de>>
    PagableDeserialize<'de> for (T1, T2, T3)
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        let m1 = T1::pagable_deserialize(deserializer)?;
        let m2 = T2::pagable_deserialize(deserializer)?;
        let m3 = T3::pagable_deserialize(deserializer)?;
        Ok((m1, m2, m3))
    }
}

impl<
    'de,
    T1: PagableDeserialize<'de>,
    T2: PagableDeserialize<'de>,
    T3: PagableDeserialize<'de>,
    T4: PagableDeserialize<'de>,
> PagableDeserialize<'de> for (T1, T2, T3, T4)
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        let m1 = T1::pagable_deserialize(deserializer)?;
        let m2 = T2::pagable_deserialize(deserializer)?;
        let m3 = T3::pagable_deserialize(deserializer)?;
        let m4 = T4::pagable_deserialize(deserializer)?;
        Ok((m1, m2, m3, m4))
    }
}

impl<
    'de,
    T1: PagableDeserialize<'de>,
    T2: PagableDeserialize<'de>,
    T3: PagableDeserialize<'de>,
    T4: PagableDeserialize<'de>,
    T5: PagableDeserialize<'de>,
> PagableDeserialize<'de> for (T1, T2, T3, T4, T5)
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        let m1 = T1::pagable_deserialize(deserializer)?;
        let m2 = T2::pagable_deserialize(deserializer)?;
        let m3 = T3::pagable_deserialize(deserializer)?;
        let m4 = T4::pagable_deserialize(deserializer)?;
        let m5 = T5::pagable_deserialize(deserializer)?;
        Ok((m1, m2, m3, m4, m5))
    }
}
