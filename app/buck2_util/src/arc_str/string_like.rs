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
use std::fmt;
use std::fmt::Display;
use std::marker::PhantomData;
use std::ops::Deref;

use allocative::Allocative;
use dupe::Clone_;
use dupe::Dupe_;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use serde::Deserialize;
use serde::Serialize;
use strong_hash::StrongHash;

use crate::arc_str::ArcStr;
use crate::arc_str::ThinArcStr;

/// Unsized type which is a string inside.
pub trait StringInside {
    /// Convert to a string.
    fn as_str(wrapper: &Self) -> &str;
    /// Convert from a string.
    fn from_str(s: &str) -> &Self;
}

impl StringInside for str {
    #[inline]
    fn as_str(wrapper: &Self) -> &str {
        wrapper
    }
    #[inline]
    fn from_str(s: &str) -> &Self {
        s
    }
}

#[derive(
    Eq, PartialEq, Ord, PartialOrd, Hash, StrongHash, Clone_, Dupe_, Debug, Allocative
)]
#[allocative(bound = "")]
pub struct ArcS<V: StringInside + ?Sized> {
    s: ArcStr,
    _marker: PhantomData<*const V>,
}

impl<S: StringInside + ?Sized> PagableSerialize for ArcS<S> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        Ok(self.serialize(serializer.serde())?)
    }
}

impl<'de, S: StringInside + ?Sized> PagableDeserialize<'de> for ArcS<S> {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        Ok(Self::deserialize(deserializer.serde())?)
    }
}

impl<S: StringInside + ?Sized> Serialize for ArcS<S> {
    fn serialize<Ser>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: serde::Serializer,
    {
        serializer.serialize_str(&self.s)
    }
}

impl<'de, S: StringInside + ?Sized> Deserialize<'de> for ArcS<S> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = String::deserialize(deserializer)?;
        Ok(ArcS::from(S::from_str(&v)))
    }
}

// Copy-paste these two lines from `std::sync::Arc`.
unsafe impl<S: StringInside + ?Sized + Sync + Send> Send for ArcS<S> {}
unsafe impl<S: StringInside + ?Sized + Sync + Send> Sync for ArcS<S> {}

impl<S: StringInside + ?Sized> ArcS<S> {
    // Cannot implement `TryFrom` trait, something about conflicting implementations.
    #[inline]
    pub fn try_from<'a>(s: &'a str) -> buck2_error::Result<ArcS<S>>
    where
        &'a S: TryFrom<&'a str, Error = buck2_error::Error>,
        S: 'a,
    {
        let s: &S = TryFrom::try_from(s)?;
        Ok(ArcS::from(s))
    }
}

impl<'a, S: StringInside + ?Sized> From<&'a S> for ArcS<S> {
    #[inline]
    fn from(s: &'a S) -> Self {
        Self {
            s: ArcStr::from(S::as_str(s)),
            _marker: PhantomData,
        }
    }
}

impl<S: StringInside + ?Sized> Deref for ArcS<S> {
    type Target = S;

    #[inline]
    fn deref(&self) -> &Self::Target {
        S::from_str(&self.s)
    }
}

impl<S: StringInside + ?Sized> AsRef<S> for ArcS<S> {
    #[inline]
    fn as_ref(&self) -> &S {
        self
    }
}

impl<S: StringInside + ?Sized> Borrow<S> for ArcS<S> {
    #[inline]
    fn borrow(&self) -> &S {
        self
    }
}

impl<S: StringInside + Display + ?Sized> Display for ArcS<S> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&**self, f)
    }
}

#[derive(
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    Clone_,
    Dupe_,
    Debug,
    Allocative,
    strong_hash::StrongHash
)]
#[allocative(bound = "")]
pub struct ThinArcS<S: StringInside + ?Sized> {
    s: ThinArcStr,
    _marker: PhantomData<*const S>,
}

// Copy-paste these two lines from `std::sync::Arc`.
unsafe impl<S: StringInside + ?Sized + Sync + Send> Send for ThinArcS<S> {}
unsafe impl<S: StringInside + ?Sized + Sync + Send> Sync for ThinArcS<S> {}

impl<S: StringInside + ?Sized> PagableSerialize for ThinArcS<S> {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        Ok(self.s.serialize(serializer.serde())?)
    }
}

impl<'de, S: StringInside + ?Sized> PagableDeserialize<'de> for ThinArcS<S> {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let v = String::deserialize(deserializer.serde())?;
        Ok(ThinArcS::from(S::from_str(&v)))
    }
}

impl<S: StringInside + ?Sized> ThinArcS<S> {
    // Cannot implement `TryFrom` trait, something about conflicting implementations.
    #[inline]
    pub fn try_from<'a>(s: &'a str) -> buck2_error::Result<ThinArcS<S>>
    where
        &'a S: TryFrom<&'a str, Error = buck2_error::Error>,
        S: 'a,
    {
        let s: &S = TryFrom::try_from(s)?;
        Ok(ThinArcS::from(s))
    }
}

impl<'a, S: StringInside + ?Sized> From<&'a S> for ThinArcS<S> {
    #[inline]
    fn from(s: &'a S) -> Self {
        Self {
            s: ThinArcStr::from(S::as_str(s)),
            _marker: PhantomData,
        }
    }
}

impl<S: StringInside + ?Sized> Deref for ThinArcS<S> {
    type Target = S;

    #[inline]
    fn deref(&self) -> &Self::Target {
        S::from_str(&self.s)
    }
}

impl<S: StringInside + ?Sized> AsRef<S> for ThinArcS<S> {
    #[inline]
    fn as_ref(&self) -> &S {
        self
    }
}

impl<S: StringInside + ?Sized> Borrow<S> for ThinArcS<S> {
    #[inline]
    fn borrow(&self) -> &S {
        self
    }
}

impl<S: StringInside + Display + ?Sized> Display for ThinArcS<S> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&**self, f)
    }
}

#[cfg(test)]
mod tests {
    use std::fmt;
    use std::fmt::Display;
    use std::hash::Hash;
    use std::hash::Hasher;

    use crate::arc_str::string_like::ArcS;
    use crate::arc_str::string_like::StringInside;

    #[derive(Hash)]
    struct MyStringWrapper(str);

    impl Display for MyStringWrapper {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "<{}>", &self.0)
        }
    }

    impl StringInside for MyStringWrapper {
        #[inline]
        fn as_str(wrapper: &Self) -> &str {
            &wrapper.0
        }
        #[inline]
        fn from_str(s: &str) -> &Self {
            unsafe { &*(s as *const str as *const Self) }
        }
    }

    impl MyStringWrapper {
        fn new(s: &str) -> &MyStringWrapper {
            unsafe { &*(s as *const str as *const Self) }
        }
    }

    #[test]
    fn test_simple() {
        let original = MyStringWrapper::new("hello");
        let s = ArcS::<MyStringWrapper>::from(original);
        let returned: &MyStringWrapper = &s;
        assert_eq!("hello", &returned.0);
    }

    #[test]
    fn test_hash_is_transparent() {
        fn hash<T: Hash + ?Sized>(t: &T) -> u64 {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            t.hash(&mut hasher);
            hasher.finish()
        }

        assert_eq!(
            hash("hello"),
            hash(&ArcS::from(MyStringWrapper::new("hello")))
        );
    }

    #[test]
    fn test_display() {
        let s = ArcS::<MyStringWrapper>::from(MyStringWrapper::new("hello"));
        assert_eq!("<hello>", format!("{s}"));
    }
}
