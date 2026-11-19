/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use serde::Deserialize;
use serde::Serialize;
use sorted_vector_map::SortedVectorMap;

use crate::traits::PagableDeserialize;
use crate::traits::PagableDeserializer;
use crate::traits::PagableSerialize;
use crate::traits::PagableSerializer;

impl<K: PagableSerialize, V: PagableSerialize> PagableSerialize
    for std::collections::BTreeMap<K, V>
{
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        usize::serialize(&self.len(), serializer.serde())?;
        for (k, v) in self {
            k.pagable_serialize(serializer)?;
            v.pagable_serialize(serializer)?;
        }
        Ok(())
    }
}

impl<'de, K: Ord + PagableDeserialize<'de>, V: PagableDeserialize<'de>> PagableDeserialize<'de>
    for std::collections::BTreeMap<K, V>
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        let items = usize::deserialize(deserializer.serde())?;
        let mut map = std::collections::BTreeMap::new();
        for _ in 0..items {
            let k = K::pagable_deserialize(deserializer)?;
            let v = V::pagable_deserialize(deserializer)?;
            map.insert(k, v);
        }
        Ok(map)
    }
}

impl<K: PagableSerialize + Ord, V: PagableSerialize> PagableSerialize
    for std::collections::HashMap<K, V>
{
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        // Serialize in sorted key order: for deterministic pagable serialization

        let mut entries: Vec<(&K, &V)> = self.iter().collect();
        entries.sort_by_key(|(a, _)| *a);
        usize::serialize(&self.len(), serializer.serde())?;
        for (k, v) in entries {
            k.pagable_serialize(serializer)?;
            v.pagable_serialize(serializer)?;
        }
        Ok(())
    }
}
impl<'de, K: std::hash::Hash + Eq + PagableDeserialize<'de>, V: PagableDeserialize<'de>>
    PagableDeserialize<'de> for std::collections::HashMap<K, V>
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        let items = usize::deserialize(deserializer.serde())?;
        let mut map = std::collections::HashMap::new();
        for _ in 0..items {
            let k = K::pagable_deserialize(deserializer)?;
            let v = V::pagable_deserialize(deserializer)?;
            map.insert(k, v);
        }
        Ok(map)
    }
}

impl<K: sequence_trie::TrieKey + PagableSerialize + Ord, V: PagableSerialize> PagableSerialize
    for sequence_trie::SequenceTrie<K, V>
{
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        // Serialize in sorted path order to keep the serialization stable.
        let mut entries: Vec<(Vec<&K>, &V)> = self.iter().collect();
        entries.sort_by(|(a, _), (b, _)| a.cmp(b));
        usize::serialize(&entries.len(), serializer.serde())?;
        for (k, v) in entries {
            <Vec<&K>>::pagable_serialize(&k, serializer)?;
            v.pagable_serialize(serializer)?;
        }
        Ok(())
    }
}
impl<'de, K: std::hash::Hash + Eq + PagableDeserialize<'de>, V: PagableDeserialize<'de>>
    PagableDeserialize<'de> for sequence_trie::SequenceTrie<K, V>
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        let items = usize::deserialize(deserializer.serde())?;
        let mut map = sequence_trie::SequenceTrie::new();
        for _ in 0..items {
            let k = Vec::<K>::pagable_deserialize(deserializer)?;
            let v = V::pagable_deserialize(deserializer)?;
            map.insert_owned(k, v);
        }
        Ok(map)
    }
}

impl<K: PagableSerialize, V: PagableSerialize> PagableSerialize for SortedVectorMap<K, V>
where
    K: Ord,
{
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        usize::serialize(&self.len(), serializer.serde())?;
        for (k, v) in self {
            k.pagable_serialize(serializer)?;
            v.pagable_serialize(serializer)?;
        }
        Ok(())
    }
}
impl<'de, K: Ord + PagableDeserialize<'de>, V: PagableDeserialize<'de>> PagableDeserialize<'de>
    for SortedVectorMap<K, V>
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        let items = usize::deserialize(deserializer.serde())?;
        let mut map = SortedVectorMap::new();
        for _ in 0..items {
            let k = K::pagable_deserialize(deserializer)?;
            let v = V::pagable_deserialize(deserializer)?;
            map.insert(k, v);
        }
        Ok(map)
    }
}

impl<K: PagableSerialize, V: PagableSerialize, H> PagableSerialize for indexmap::IndexMap<K, V, H> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        usize::serialize(&self.len(), serializer.serde())?;
        for (k, v) in self {
            k.pagable_serialize(serializer)?;
            v.pagable_serialize(serializer)?;
        }
        Ok(())
    }
}

impl<
    'de,
    K: std::hash::Hash + Eq + PagableDeserialize<'de>,
    V: PagableDeserialize<'de>,
    H: std::default::Default + std::hash::BuildHasher,
> PagableDeserialize<'de> for indexmap::IndexMap<K, V, H>
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        let items = usize::deserialize(deserializer.serde())?;
        let mut map: indexmap::IndexMap<K, V, H> = indexmap::IndexMap::default();
        for _ in 0..items {
            let k = K::pagable_deserialize(deserializer)?;
            let v = V::pagable_deserialize(deserializer)?;
            map.insert(k, v);
        }
        Ok(map)
    }
}

impl<V: PagableSerialize> PagableSerialize for std::collections::BTreeSet<V> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        usize::serialize(&self.len(), serializer.serde())?;
        for v in self {
            v.pagable_serialize(serializer)?;
        }
        Ok(())
    }
}
impl<'de, V: Ord + PagableDeserialize<'de>> PagableDeserialize<'de>
    for std::collections::BTreeSet<V>
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        let items = usize::deserialize(deserializer.serde())?;
        let mut set = std::collections::BTreeSet::new();
        for _ in 0..items {
            let v = V::pagable_deserialize(deserializer)?;
            set.insert(v);
        }
        Ok(set)
    }
}

impl<V: PagableSerialize> PagableSerialize for indexmap::IndexSet<V> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        usize::serialize(&self.len(), serializer.serde())?;
        for v in self {
            v.pagable_serialize(serializer)?;
        }
        Ok(())
    }
}
impl<'de, V: std::hash::Hash + Eq + PagableDeserialize<'de>> PagableDeserialize<'de>
    for indexmap::IndexSet<V>
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        let items = usize::deserialize(deserializer.serde())?;
        let mut set = indexmap::IndexSet::new();
        for _ in 0..items {
            let v = V::pagable_deserialize(deserializer)?;
            set.insert(v);
        }
        Ok(set)
    }
}

impl<T: PagableSerialize> PagableSerialize for Vec<T> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        T::pagable_serialize_vec(self, serializer)
    }
}

impl<'de, T: PagableDeserialize<'de>> PagableDeserialize<'de> for Vec<T> {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        T::pagable_deserialize_vec(deserializer)
    }
}

impl<T: PagableSerialize, const N: usize> PagableSerialize for smallvec::SmallVec<[T; N]> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        usize::serialize(&self.len(), serializer.serde())?;
        for v in self {
            v.pagable_serialize(serializer)?;
        }
        Ok(())
    }
}

impl<'de, T: PagableDeserialize<'de>, const N: usize> PagableDeserialize<'de>
    for smallvec::SmallVec<[T; N]>
{
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        let items = usize::deserialize(deserializer.serde())?;
        let mut v = smallvec::SmallVec::with_capacity(items);
        for _ in 0..items {
            v.push(T::pagable_deserialize(deserializer)?);
        }
        Ok(v)
    }
}

impl<T: PagableSerialize> PagableSerialize for &[T] {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        for v in *self {
            v.pagable_serialize(serializer)?;
        }
        Ok(())
    }
}

macro_rules! array_impls {
    ($($len:expr => ($($n:tt)+))+) => {
        $(
            impl<T: PagableSerialize> PagableSerialize for [T; $len] {
                fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer)-> crate::Result<()> {
                    for v in self {
                        v.pagable_serialize(serializer)?;
                    }
                    Ok(())
                }
            }

            impl<'de, T: PagableDeserialize<'de> + Sized> PagableDeserialize<'de> for [T; $len] {
                fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
                    deserializer: &mut D,
                ) -> crate::Result<Self> {
                    Ok([$(
                        match T::pagable_deserialize(deserializer) {
                            Ok(val) => val,
                            Err(_e) => return Err(anyhow::anyhow!("Failed to deserialize array {}", $n)),
                        }
                    ),+])
                }
            }
        )+
    }
}

array_impls! {
    1 => (0)
    2 => (0 1)
    3 => (0 1 2)
    4 => (0 1 2 3)
    5 => (0 1 2 3 4)
    6 => (0 1 2 3 4 5)
    7 => (0 1 2 3 4 5 6)
    8 => (0 1 2 3 4 5 6 7)
    9 => (0 1 2 3 4 5 6 7 8)
    10 => (0 1 2 3 4 5 6 7 8 9)
    11 => (0 1 2 3 4 5 6 7 8 9 10)
    12 => (0 1 2 3 4 5 6 7 8 9 10 11)
    13 => (0 1 2 3 4 5 6 7 8 9 10 11 12)
    14 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13)
    15 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14)
    16 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15)
    17 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16)
    18 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17)
    19 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18)
    20 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19)
    21 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20)
    22 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21)
    23 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22)
    24 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23)
    25 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24)
    26 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25)
    27 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26)
    28 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27)
    29 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28)
    30 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29)
    31 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30)
    32 => (0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::testing::TestingDeserializer;
    use crate::testing::TestingSerializer;
    use crate::traits::PagableDeserialize;
    use crate::traits::PagableSerialize;

    /// Keeps the default sequence hooks, so a `Vec<PlainByte>` encodes the way a
    /// `Vec<u8>` would have before `u8` overrode them.
    #[derive(Debug, PartialEq)]
    struct PlainByte(u8);

    impl PagableSerialize for PlainByte {
        fn pagable_serialize(
            &self,
            serializer: &mut dyn crate::PagableSerializer,
        ) -> crate::Result<()> {
            self.0.pagable_serialize(serializer)
        }
    }

    impl<'de> PagableDeserialize<'de> for PlainByte {
        fn pagable_deserialize<D: crate::PagableDeserializer<'de> + ?Sized>(
            deserializer: &mut D,
        ) -> crate::Result<Self> {
            Ok(PlainByte(u8::pagable_deserialize(deserializer)?))
        }
    }

    /// `u8` takes a `Vec` as one slice, only sound while that encodes exactly
    /// what the default would. A round trip cannot catch a divergence - both
    /// sides move together - it would misparse across binary versions instead.
    #[test]
    fn test_vec_u8_encodes_as_the_default_would() -> crate::Result<()> {
        // 128 is where postcard's varint length grows a second byte, so the
        // prefix is exercised in both widths.
        for len in [0usize, 1, 127, 128, 300] {
            let bytes: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let plain: Vec<PlainByte> = bytes.iter().copied().map(PlainByte).collect();

            let mut serializer = TestingSerializer::new();
            bytes.pagable_serialize(&mut serializer)?;
            let as_bytes = serializer.finish();

            let mut serializer = TestingSerializer::new();
            plain.pagable_serialize(&mut serializer)?;
            let as_elements = serializer.finish();

            assert_eq!(
                as_bytes, as_elements,
                "Vec<u8> of {len} bytes did not encode the way the default path does"
            );

            let mut deserializer = TestingDeserializer::new(&as_bytes);
            let restored: Vec<u8> = Vec::pagable_deserialize(&mut deserializer)?;
            assert_eq!(
                bytes, restored,
                "Vec<u8> of {len} bytes lost its round trip"
            );

            let mut deserializer = TestingDeserializer::new(&as_elements);
            let restored: Vec<u8> = Vec::pagable_deserialize(&mut deserializer)?;
            assert_eq!(
                bytes, restored,
                "Vec<u8> of {len} bytes could not read what the default path wrote"
            );

            // The other direction: a reader without the `u8` override, on bytes
            // written with it.
            let mut deserializer = TestingDeserializer::new(&as_bytes);
            let restored: Vec<PlainByte> = Vec::pagable_deserialize(&mut deserializer)?;
            assert_eq!(
                plain, restored,
                "Vec<u8> of {len} bytes could not be read by the default path"
            );
        }
        Ok(())
    }

    #[test]
    fn test_hashmap_roundtrip() -> crate::Result<()> {
        let mut map: HashMap<String, i32> = HashMap::new();
        map.insert("one".to_owned(), 1);
        map.insert("two".to_owned(), 2);
        map.insert("three".to_owned(), 3);

        let mut serializer = TestingSerializer::new();
        map.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: HashMap<String, i32> = HashMap::pagable_deserialize(&mut deserializer)?;

        assert_eq!(map, restored);
        Ok(())
    }

    #[test]
    fn test_sequence_trie_roundtrip() -> crate::Result<()> {
        use sequence_trie::SequenceTrie;

        let mut trie: SequenceTrie<String, i32> = SequenceTrie::new();
        trie.insert(&["a".to_owned(), "b".to_owned()], 1);
        trie.insert(&["a".to_owned(), "c".to_owned()], 2);
        trie.insert(&["d".to_owned()], 3);

        let mut serializer = TestingSerializer::new();
        trie.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: SequenceTrie<String, i32> =
            SequenceTrie::pagable_deserialize(&mut deserializer)?;

        // SequenceTrie doesn't implement Eq, so compare by iterating
        let original: Vec<_> = trie.iter().map(|(k, v)| (k, *v)).collect();
        let restored_items: Vec<_> = restored.iter().map(|(k, v)| (k, *v)).collect();
        assert_eq!(original.len(), restored_items.len());
        for (k, v) in &original {
            assert_eq!(restored.get(k.iter().map(|s| s.as_str())), Some(v));
        }
        Ok(())
    }

    #[test]
    fn test_sorted_vector_map_roundtrip() -> crate::Result<()> {
        use sorted_vector_map::SortedVectorMap;

        let mut map: SortedVectorMap<String, i32> = SortedVectorMap::new();
        map.insert("one".to_owned(), 1);
        map.insert("two".to_owned(), 2);
        map.insert("three".to_owned(), 3);

        let mut serializer = TestingSerializer::new();
        map.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: SortedVectorMap<String, i32> =
            SortedVectorMap::pagable_deserialize(&mut deserializer)?;

        assert_eq!(map, restored);
        Ok(())
    }

    #[test]
    fn test_btreeset_roundtrip() -> crate::Result<()> {
        use std::collections::BTreeSet;

        let mut set: BTreeSet<String> = BTreeSet::new();
        set.insert("one".to_owned());
        set.insert("two".to_owned());
        set.insert("three".to_owned());

        let mut serializer = TestingSerializer::new();
        set.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: BTreeSet<String> = BTreeSet::pagable_deserialize(&mut deserializer)?;

        assert_eq!(set, restored);
        Ok(())
    }

    #[test]
    fn test_indexmap_roundtrip() -> crate::Result<()> {
        use indexmap::IndexMap;

        let mut map: IndexMap<String, i32> = IndexMap::new();
        map.insert("one".to_owned(), 1);
        map.insert("two".to_owned(), 2);
        map.insert("three".to_owned(), 3);

        let mut serializer = TestingSerializer::new();
        map.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: IndexMap<String, i32> = IndexMap::pagable_deserialize(&mut deserializer)?;

        assert_eq!(map, restored);
        Ok(())
    }

    #[test]
    fn test_indexset_roundtrip() -> crate::Result<()> {
        use indexmap::IndexSet;

        let mut set: IndexSet<String> = IndexSet::new();
        set.insert("one".to_owned());
        set.insert("two".to_owned());
        set.insert("three".to_owned());

        let mut serializer = TestingSerializer::new();
        set.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: IndexSet<String> = IndexSet::pagable_deserialize(&mut deserializer)?;

        assert_eq!(set, restored);
        Ok(())
    }

    #[test]
    fn test_smallvec_roundtrip() -> crate::Result<()> {
        use smallvec::SmallVec;

        let mut vec: SmallVec<[String; 4]> = SmallVec::new();
        vec.push("one".to_owned());
        vec.push("two".to_owned());
        vec.push("three".to_owned());

        let mut serializer = TestingSerializer::new();
        vec.pagable_serialize(&mut serializer)?;
        let bytes = serializer.finish();

        let mut deserializer = TestingDeserializer::new(&bytes);
        let restored: SmallVec<[String; 4]> = SmallVec::pagable_deserialize(&mut deserializer)?;

        assert_eq!(vec, restored);
        Ok(())
    }
}
