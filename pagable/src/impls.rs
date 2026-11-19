/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

mod arc;
mod blake3;
mod collections;
mod regex;
mod serde;
mod static_interner;
mod std;
mod tuples;

pub mod static_value;
use num_bigint::BigInt;
pub use static_value::StaticStr;

use crate::PagableDeserialize;
use crate::PagableDeserializer;
use crate::PagableSerialize;
use crate::PagableSerializer;

impl PagableSerialize for BigInt {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        use ::serde::Serialize;
        Ok(self.serialize(serializer.serde())?)
    }
}

impl<'de> PagableDeserialize<'de> for BigInt {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        use ::serde::Deserialize;
        Ok(Deserialize::deserialize(deserializer.serde())?)
    }
}

#[cfg(test)]
mod tests {
    use num_bigint::BigInt;

    use crate::testing::TestingDeserializer;
    use crate::testing::TestingSerializer;
    use crate::traits::PagableDeserialize;
    use crate::traits::PagableSerialize;

    #[test]
    fn test_bigint_roundtrip() -> crate::Result<()> {
        let value = BigInt::from(123456789012345678901234567890_i128);
        let mut serializer = TestingSerializer::new();
        value.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();
        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored = BigInt::pagable_deserialize(&mut deserializer)?;
        assert_eq!(value, restored);
        Ok(())
    }
}
