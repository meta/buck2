/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::hash::Hash;
use std::hash::Hasher;
use std::marker::PhantomData;
use std::sync::Arc;

use ref_cast::RefCast;
pub use strong_hash_derive::StrongHash;

use crate as strong_hash;

mod impls;

/// `StrongHash`` is a trait that is notionally similar to `std::hash::Hash`, but carries the
/// implicit expectation that the hash that will be produced should be as perturbed as possible.
///
/// `StrongHash` must be implemented such that - when combined with a crypto hasher - the hash value
/// is suitable to use for equality checks. That means fields must not be omitted and pre-computed
/// hash values cannot be used if the hasher used for them is weak.
///
/// `StrongHash` can be derived on enums and structs if all of their members implement `StrongHash`.
/// For example:
///
/// ```ignore
/// #[derive(StrongHash)]
/// struct MyStruct {
///     a: u32,
///     b: String,
///     c: Vec<u8>,
/// }
/// ```
///
/// `StrongHash` can also be implemented manually similar to std::hash::Hash. For example:
///
/// ```ignore
/// struct MyStruct {
///     a: u32,
///     b: String,
///     c: Vec<u8>,
/// }
///
/// impl StrongHash for MyStruct {
///     fn strong_hash<H: StrongHasher>(&self, state: &mut H) {
///         self.a.strong_hash(state);
///         self.b.strong_hash(state);
///         self.c.strong_hash(state);
///     }
/// }
/// ```
pub trait StrongHash {
    fn strong_hash<H: Hasher>(&self, state: &mut H);
}

#[macro_export]
macro_rules! impl_strong_hash_for_impl_hash {
    ($($t:ty)*) => {
        $(
            impl strong_hash::StrongHash for $t {
                fn strong_hash<H: std::hash::Hasher>(&self, state: &mut H) {
                    std::hash::Hash::hash(self, state);
                }
            }
        )*
    };
}

impl_strong_hash_for_impl_hash!(bool u8 i8 u16 i16 u32 i32 u64 i64 usize str String);

impl<T: StrongHash + ?Sized> StrongHash for &T {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        (**self).strong_hash(state);
    }
}

impl<T: StrongHash> StrongHash for [T] {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.len().strong_hash(state);
        for item in self.iter() {
            item.strong_hash(state);
        }
    }
}

impl<T: StrongHash> StrongHash for Vec<T> {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.len().strong_hash(state);
        self[..].strong_hash(state);
    }
}

impl<T: StrongHash> StrongHash for Option<T> {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.is_some().strong_hash(state);
        if let Some(t) = self.as_ref() {
            t.strong_hash(state);
        }
    }
}

impl StrongHash for () {
    fn strong_hash<H: Hasher>(&self, _state: &mut H) {}
}

impl<A: StrongHash> StrongHash for (A,) {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        1.strong_hash(state);
        self.0.strong_hash(state);
    }
}

impl<A: StrongHash, B: StrongHash> StrongHash for (A, B) {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        2.strong_hash(state);
        self.0.strong_hash(state);
        self.1.strong_hash(state);
    }
}

impl<A: StrongHash, B: StrongHash, C: StrongHash> StrongHash for (A, B, C) {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        3.strong_hash(state);
        self.0.strong_hash(state);
        self.1.strong_hash(state);
        self.2.strong_hash(state);
    }
}

impl<T: StrongHash + ?Sized> StrongHash for Box<T> {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.as_ref().strong_hash(state);
    }
}

impl<T: StrongHash + ?Sized> StrongHash for Arc<T> {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.as_ref().strong_hash(state);
    }
}

impl<K: StrongHash, V: StrongHash> StrongHash for BTreeMap<K, V> {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.len().strong_hash(state);
        for (k, v) in self.iter() {
            k.strong_hash(state);
            v.strong_hash(state);
        }
    }
}

// There is deliberately no `StrongHash` for `HashMap`: its iteration order is
// nondeterministic, so two equal maps can produce different hashes, violating
// this trait's equality contract. Use `BTreeMap`, or hash entries in an
// explicitly canonical order.

impl StrongHash for *const () {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        (*self as usize).strong_hash(state);
    }
}

impl<T: ?Sized> StrongHash for PhantomData<T> {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.hash(state);
    }
}

/// A wrapper can be used to implement `Hash` using the inner type's `StrongHash`.
#[derive(RefCast)]
#[repr(transparent)]
pub struct UseStrongHashing<T: ?Sized>(pub T);

impl<T> Hash for UseStrongHashing<T>
where
    T: ?Sized + StrongHash,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.strong_hash(state);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    struct TestHashable {
        hash: u64,
        strong_hash: u64,
    }

    impl Hash for TestHashable {
        fn hash<H: Hasher>(&self, state: &mut H) {
            self.hash.hash(state);
        }
    }

    impl StrongHash for TestHashable {
        fn strong_hash<H: Hasher>(&self, state: &mut H) {
            self.strong_hash.strong_hash(state);
        }
    }

    fn hash<T: Hash>(t: &T) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        t.hash(&mut hasher);
        hasher.finish()
    }

    fn strong_hash<T: StrongHash>(t: &T) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        t.strong_hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn test_use_strong_hashing() {
        let x = TestHashable {
            hash: 1,
            strong_hash: 2,
        };

        let y = TestHashable {
            hash: 1,
            strong_hash: 3,
        };

        assert_eq!(hash(&x), hash(&y));

        assert_eq!(strong_hash(&x), hash(UseStrongHashing::ref_cast(&x)));

        assert_ne!(
            hash(UseStrongHashing::ref_cast(&x)),
            hash(UseStrongHashing::ref_cast(&y))
        );
    }
}
