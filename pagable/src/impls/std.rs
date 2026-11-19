/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use core::sync::atomic::AtomicI64;
use std::sync::OnceLock;

use ::serde::Deserialize;
use ::serde::ser::Serialize;

use crate::traits::PagableBoxDeserialize;
use crate::traits::PagableDeserialize;
use crate::traits::PagableDeserializer;
use crate::traits::PagableSerialize;
use crate::traits::PagableSerializer;

impl PagableSerialize for () {
    fn pagable_serialize(&self, _serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        Ok(())
    }
}

impl<'de> PagableDeserialize<'de> for () {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        _deserializer: &mut D,
    ) -> crate::Result<Self> {
        Ok(())
    }
}

impl<'a, T: PagableSerialize> PagableSerialize for &'a T {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        (**self).pagable_serialize(serializer)
    }
}

impl<T: PagableSerialize> PagableSerialize for Option<T> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        bool::serialize(&self.is_some(), serializer.serde())?;
        if let Some(v) = self {
            v.pagable_serialize(serializer)?;
        }
        Ok(())
    }
}

impl<'de, T: PagableDeserialize<'de>> PagableDeserialize<'de> for Option<T> {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        if bool::deserialize(deserializer.serde())? {
            Ok(Some(T::pagable_deserialize(deserializer)?))
        } else {
            Ok(None)
        }
    }
}

impl<T: PagableSerialize> PagableSerialize for OnceLock<T> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        match self.get() {
            Some(value) => {
                bool::serialize(&true, serializer.serde())?;
                value.pagable_serialize(serializer)?;
            }
            None => {
                bool::serialize(&false, serializer.serde())?;
            }
        }
        Ok(())
    }
}

impl<'de, T: PagableDeserialize<'de>> PagableDeserialize<'de> for OnceLock<T> {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        let value = OnceLock::new();
        if bool::deserialize(deserializer.serde())? {
            let set = value.set(T::pagable_deserialize(deserializer)?);
            if set.is_err() {
                unreachable!("new OnceLock should not already be initialized");
            }
        }
        Ok(value)
    }
}

impl<T: ?Sized> PagableSerialize for std::marker::PhantomData<T> {
    fn pagable_serialize(&self, _serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        Ok(())
    }
}

impl<'de, T: ?Sized> PagableDeserialize<'de> for std::marker::PhantomData<T> {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        _deserializer: &mut D,
    ) -> crate::Result<Self> {
        Ok(std::marker::PhantomData)
    }
}

impl<T: PagableSerialize, E: PagableSerialize> PagableSerialize for Result<T, E> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        match self {
            Ok(v) => {
                bool::serialize(&true, serializer.serde())?;
                v.pagable_serialize(serializer)?;
            }
            Err(e) => {
                bool::serialize(&false, serializer.serde())?;
                e.pagable_serialize(serializer)?;
            }
        }
        Ok(())
    }
}

impl<'de, T: PagableDeserialize<'de>, E: PagableDeserialize<'de>> PagableDeserialize<'de>
    for Result<T, E>
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        if bool::deserialize(deserializer.serde())? {
            Ok(Ok(T::pagable_deserialize(deserializer)?))
        } else {
            Ok(Err(E::pagable_deserialize(deserializer)?))
        }
    }
}

impl<T: ?Sized + PagableSerialize> PagableSerialize for Box<T> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        (**self).pagable_serialize(serializer)
    }
}

impl<'de, T: ?Sized + PagableBoxDeserialize<'de>> PagableDeserialize<'de> for Box<T> {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        T::deserialize_box(deserializer)
    }
}

impl<'de, T: PagableDeserialize<'de>> PagableBoxDeserialize<'de> for T {
    fn deserialize_box<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Box<Self>> {
        let val = Self::pagable_deserialize(deserializer)?;
        Ok(Box::new(val))
    }
}

impl<T: PagableSerialize> PagableSerialize for [T] {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        usize::serialize(&self.len(), serializer.serde())?;
        for v in self.iter() {
            v.pagable_serialize(serializer)?;
        }
        Ok(())
    }
}

impl<'de, T: PagableDeserialize<'de>> PagableBoxDeserialize<'de> for [T] {
    fn deserialize_box<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Box<Self>> {
        let items = usize::deserialize(deserializer.serde())?;
        let mut v = Vec::with_capacity(items);
        for _ in 0..items {
            v.push(T::pagable_deserialize(deserializer)?);
        }
        Ok(v.into_boxed_slice())
    }
}

impl PagableSerialize for AtomicI64 {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        Ok(i64::serialize(
            &self.load(core::sync::atomic::Ordering::Relaxed),
            serializer.serde(),
        )?)
    }
}

impl<'de> PagableDeserialize<'de> for AtomicI64 {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        let val = i64::deserialize(deserializer.serde())?;
        Ok(AtomicI64::new(val))
    }
}

impl PagableSerialize for str {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        Ok(self.serialize(serializer.serde())?)
    }
}

impl<'de> PagableBoxDeserialize<'de> for str {
    fn deserialize_box<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Box<Self>> {
        let s = String::deserialize(deserializer.serde())?;
        Ok(s.into_boxed_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestingDeserializer;
    use crate::testing::TestingSerializer;

    #[test]
    fn test_result_roundtrip() -> crate::Result<()> {
        // Test Ok variant
        let ok_result: Result<String, String> = Ok("success".to_owned());
        let mut serializer = TestingSerializer::new();
        ok_result.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();
        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: Result<String, String> = Result::pagable_deserialize(&mut deserializer)?;
        assert_eq!(ok_result, restored);

        // Test Err variant
        let err_result: Result<String, String> = Err("failure".to_owned());
        let mut serializer = TestingSerializer::new();
        err_result.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();
        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: Result<String, String> = Result::pagable_deserialize(&mut deserializer)?;
        assert_eq!(err_result, restored);

        Ok(())
    }
}
