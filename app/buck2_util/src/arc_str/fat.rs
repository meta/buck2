/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Borrow;
use std::fmt::Debug;
use std::fmt::Display;
use std::hash::Hash;
use std::ops::Deref;
use std::str;
use std::sync::Arc;

use allocative::Allocative;
use dupe::Dupe;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use pagable::arc_erase::ArcErase;
use pagable::arc_erase::ArcEraseType;
use pagable::arc_erase::ArcSerializeOutcome;
use pagable::arc_erase::StdArcEraseType;
use pagable::arc_erase::deserialize_arc;
use serde::Deserialize;
use serde::Serialize;
use strong_hash::StrongHash;

use crate::arc_str::base::ArcStrBase;
use crate::arc_str::base::ArcStrBaseInner;
use crate::arc_str::base::ArcStrLenStrategy;
use crate::size_assert;

struct ArcStrProperties;

unsafe impl ArcStrLenStrategy for ArcStrProperties {
    type AllocatedPayload = ();
    type ValuePayload = u32;
    const EMPTY: (Self::AllocatedPayload, Self::ValuePayload) = ((), 0);

    #[inline]
    fn unpack_len(s: (Self::AllocatedPayload, Self::ValuePayload)) -> u32 {
        s.1
    }

    #[inline]
    fn inner_empty() -> &'static ArcStrBaseInner<Self> {
        &INNER_EMPTY
    }

    #[inline]
    fn pack_len(len: u32) -> (Self::AllocatedPayload, Self::ValuePayload) {
        ((), len)
    }
}

/// Wrapper for `Arc<str>`.
#[derive(
    PartialEq, Eq, Hash, StrongHash, PartialOrd, Ord, Allocative, Clone, Dupe, Default
)]
pub struct ArcStr {
    base: ArcStrBase<ArcStrProperties>,
}

size_assert::same_size!(ArcStr, Arc<str>);
size_assert::same_size!(Option<ArcStr>, Arc<str>);

static INNER_EMPTY: ArcStrBaseInner<ArcStrProperties> = ArcStrBaseInner::empty();

impl ArcStr {
    #[inline]
    pub fn as_str(&self) -> &str {
        self.base.as_str()
    }
}

impl Deref for ArcStr {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for ArcStr {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for ArcStr {
    #[inline]
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl Display for ArcStr {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self.as_str(), f)
    }
}

impl Debug for ArcStr {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(self.as_str(), f)
    }
}

impl<'a> From<&'a str> for ArcStr {
    #[inline]
    fn from(s: &'a str) -> ArcStr {
        ArcStr {
            base: ArcStrBase::from(s),
        }
    }
}

impl From<String> for ArcStr {
    #[inline]
    fn from(s: String) -> ArcStr {
        ArcStr::from(s.as_str())
    }
}

impl Serialize for ArcStr {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_str().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ArcStr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(String::deserialize(deserializer)?.into())
    }
}

impl PagableSerialize for ArcStr {
    fn pagable_serialize(
        &self,
        serializer: &mut dyn PagableSerializer,
    ) -> pagable::__internal::anyhow::Result<()> {
        serializer.serialize_arc(&self.dupe())
    }
}

impl<'de> PagableDeserialize<'de> for ArcStr {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        deserialize_arc::<Self, _>(deserializer)
    }
}

impl ArcErase for ArcStr {
    type Weak = ();

    fn dupe_strong(&self) -> Self {
        self.dupe()
    }

    fn erase_type() -> impl ArcEraseType {
        StdArcEraseType::<Self>::new()
    }

    fn identity(&self) -> usize {
        self.base.addr()
    }

    fn downgrade(&self) -> Option<Self::Weak> {
        None
    }

    fn is_unique(&self) -> bool {
        self.base.is_unique()
    }

    fn serialize_inner(
        &self,
        ser: &mut dyn PagableSerializer,
    ) -> pagable::__internal::anyhow::Result<ArcSerializeOutcome> {
        Serialize::serialize(&self, ser.serde())?;
        Ok(ArcSerializeOutcome::Serialized)
    }

    fn deserialize_inner<'de, D: PagableDeserializer<'de> + ?Sized>(
        deser: &mut D,
    ) -> pagable::__internal::anyhow::Result<Self> {
        Ok(Self::deserialize(deser.serde())?)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hash;
    use std::hash::Hasher;
    use std::sync::atomic;

    use dupe::Dupe;
    use pagable::arc_erase::ArcErase;

    use crate::arc_str::ArcStr;

    #[test]
    fn test_serialize() {
        assert_eq!(
            "\"hello\"",
            serde_json::to_string(&ArcStr::from("hello")).unwrap()
        );
        assert_eq!("\"\"", serde_json::to_string(&ArcStr::from("")).unwrap());
    }

    #[test]
    fn test_hash() {
        fn hash<H: Hash + ?Sized>(h: &H) -> u64 {
            let mut hasher = DefaultHasher::new();
            h.hash(&mut hasher);
            hasher.finish()
        }

        assert_eq!(hash(""), hash(&ArcStr::from("")));
        assert_eq!(hash("hello"), hash(&ArcStr::from("hello")));
    }

    #[test]
    fn test_clone_drop_ref_count() {
        fn ref_count(s: &ArcStr) -> u32 {
            assert!(!s.is_empty());
            s.base.inner().refcount.load(atomic::Ordering::Relaxed)
        }

        let s = ArcStr::from("hello");
        assert_eq!(1, ref_count(&s));
        let s2 = s.dupe();
        assert_eq!(2, ref_count(&s));
        drop(s);
        assert_eq!(1, ref_count(&s2));
    }

    #[test]
    fn test_is_unique() {
        let s = ArcStr::from("hello");
        assert!(ArcErase::is_unique(&s));
        let s2 = s.dupe();
        assert!(!ArcErase::is_unique(&s));
        drop(s2);
        assert!(ArcErase::is_unique(&s));
        assert!(!ArcErase::is_unique(&ArcStr::from("")));
    }

    #[test]
    fn test_little_stress_test() {
        // Interpreted, a string costs milliseconds on the Windows CI hosts.
        let (rounds, churn) = if cfg!(miri) { (20, 100) } else { (100, 1000) };
        for i in 0..rounds {
            let strings = (0..i)
                .map(|i| ArcStr::from(i.to_string()))
                .collect::<Vec<_>>();
            for j in 0..churn {
                let _ignore = ArcStr::from(j.to_string());
            }
            for (k, s) in strings.iter().enumerate() {
                assert_eq!(k.to_string(), s.as_str());
            }
        }
    }
}
