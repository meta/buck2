/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use blake3::Hash;

use crate::PagableDeserialize;
use crate::PagableDeserializer;
use crate::PagableSerialize;
use crate::PagableSerializer;

impl PagableSerialize for Hash {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        self.as_bytes().pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for Hash {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        Ok(Hash::from_bytes(<[u8; 32]>::pagable_deserialize(
            deserializer,
        )?))
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::TestingDeserializer;
    use crate::testing::TestingSerializer;
    use crate::traits::PagableDeserialize;
    use crate::traits::PagableSerialize;

    #[test]
    fn test_blake3_hash_roundtrip() -> crate::Result<()> {
        let value = blake3::hash(b"some bytes");
        let mut serializer = TestingSerializer::new();
        value.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();
        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored = blake3::Hash::pagable_deserialize(&mut deserializer)?;
        assert_eq!(value, restored);
        Ok(())
    }
}
