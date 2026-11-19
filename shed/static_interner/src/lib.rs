/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Intern objects in memory.
//!
//! This is similar to [`internment` crate](https://github.com/droundy/internment)
//! but with changes for performance and flexibility.

use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::hash::Hash;
use std::hash::Hasher;
use std::marker::PhantomData;
use std::ops::Deref;
use std::ptr;

pub use equivalent::Equivalent;
use lock_free_hashtable::sharded::ShardedLockFreeRawTable;

pub struct Interner<T: 'static, H = DefaultHasher> {
    table: ShardedLockFreeRawTable<Box<InternedData<T>>, 64>,
    _marker: PhantomData<H>,
}

/// This structure is similar to `Hashed<T>`, but it is not parameterized by hash function.
#[derive(Debug)]
struct InternedData<T: 'static> {
    data: T,
    hash: u64,
}

/// An interned pointer.
///
/// Equality of this type is a pointer comparison.
/// But note, this works correctly only if `Intern` pointers created
/// from the same instance of `Interner`.
#[derive(Debug)]
pub struct Intern<T: 'static> {
    pointer: &'static InternedData<T>,
}

pub trait Internable {
    type Hasher;

    fn interner() -> &'static Interner<Self, Self::Hasher>
    where
        Self: Sized;
}

#[cfg(feature = "strong_hash")]
impl<T: strong_hash::StrongHash> strong_hash::StrongHash for Intern<T> {
    fn strong_hash<H: Hasher>(&self, hasher: &mut H) {
        self.pointer.data.strong_hash(hasher);
    }
}

// TODO(nga): derive.
#[cfg(feature = "allocative")]
impl<T: allocative::Allocative> allocative::Allocative for Intern<T> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        if std::mem::size_of::<T>() > 0 {
            let visitor = visitor.enter_shared(
                allocative::Key::new("pointer"),
                std::mem::size_of::<*const T>(),
                &**self as &T as *const T as *const (),
            );
            if let Some(mut visitor) = visitor {
                (**self).visit(&mut visitor);
                visitor.exit();
            }
        }
    }
}

impl<T: 'static> Copy for Intern<T> {}

impl<T: 'static> Clone for Intern<T> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

#[cfg(feature = "dupe")]
impl<T: 'static> dupe::Dupe for Intern<T> {
    #[inline]
    fn dupe(&self) -> Self {
        *self
    }
}

impl<T: 'static> Deref for Intern<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        &self.pointer.data
    }
}

impl<T: 'static> Intern<T> {
    #[inline]
    pub const fn deref_static(&self) -> &'static T {
        &self.pointer.data
    }

    /// The interned value as a pointer, for storing an `Intern` as one. Recover the `Intern`
    /// with [`Self::from_ptr`]. A pointer taken from [`Self::deref_static`] would not do for
    /// that: it has provenance over the `T` alone, not the allocation around it.
    #[inline]
    pub const fn as_ptr(&self) -> *const T {
        &raw const self.pointer.data
    }

    /// # Safety
    ///
    /// `p` must have been returned by [`Self::as_ptr`].
    #[inline]
    pub const unsafe fn from_ptr(p: *const T) -> Self {
        // SAFETY: `p` points at the `data` field of an `InternedData<T>`, with provenance over
        // the whole `InternedData<T>`.
        unsafe {
            let p = p
                .cast::<u8>()
                .sub(std::mem::offset_of!(InternedData<T>, data))
                .cast::<InternedData<T>>();
            Self { pointer: &*p }
        }
    }
}

impl<T> Hash for Intern<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // We could hash only the pointer, since we only compare the pointers,
        // but users may expect hashing to be stable between runs.
        self.pointer.hash.hash(state);
    }
}

impl<T> PartialEq for Intern<T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self.pointer, other.pointer)
    }
}

impl<T> Eq for Intern<T> {}

impl<T: PartialOrd> PartialOrd for Intern<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.pointer.data.partial_cmp(&other.pointer.data)
    }
}

impl<T: Ord> Ord for Intern<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.pointer.data.cmp(&other.pointer.data)
    }
}

impl<T: Display> Display for Intern<T> {
    #[inline]
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        Display::fmt(&self.pointer.data, f)
    }
}

/// Hash the value before acquiring the lock.
struct Hashed<T, H> {
    hash: u64,
    value: T,
    _marker: PhantomData<H>,
}

impl<T: Hash, H: Hasher + Default> Hashed<T, H> {
    /// Compute the hash.
    fn hash(value: &T) -> u64 {
        let mut hasher = H::default();
        value.hash(&mut hasher);
        hasher.finish()
    }

    fn new(value: T) -> Self {
        let hash = Self::hash(&value);
        Hashed {
            hash,
            value,
            _marker: PhantomData,
        }
    }
}

impl<T: 'static, H> Interner<T, H> {
    /// Create a new interner for given type.
    pub const fn new() -> Interner<T, H> {
        Interner {
            table: ShardedLockFreeRawTable::new(),
            _marker: PhantomData,
        }
    }
}

pub enum InternDisposition {
    /// The value was already interned.
    Interned,
    /// The value was computed and interned.
    Computed,
}

impl<T: 'static, H: Hasher + Default> Interner<T, H> {
    /// Allocate a value, or return previously allocated one as well as whether that value
    /// was computed or already interned.
    pub fn observed_intern<Q>(&'static self, value: Q) -> (Intern<T>, InternDisposition)
    where
        Q: Hash + Equivalent<T> + Into<T>,
        T: Eq + Hash,
    {
        let hashed = Hashed::<_, H>::new(value);
        if let Some(pointer) = self
            .table
            .lookup(hashed.hash, |t| hashed.value.equivalent(&t.data))
        {
            return (Intern { pointer }, InternDisposition::Interned);
        }

        self.intern_slow(hashed)
    }

    /// Allocate a value, or return previously allocated one.
    pub fn intern<Q>(&'static self, value: Q) -> Intern<T>
    where
        Q: Hash + Equivalent<T> + Into<T>,
        T: Eq + Hash,
    {
        let (intern, _disposition) = self.observed_intern(value);
        intern
    }

    #[cold]
    fn intern_slow<Q>(&'static self, hashed_value: Hashed<Q, H>) -> (Intern<T>, InternDisposition)
    where
        Q: Hash + Equivalent<T> + Into<T>,
        T: Eq + Hash,
    {
        let pointer = Box::new(InternedData {
            data: hashed_value.value.into(),
            hash: hashed_value.hash,
        });
        let (pointer, existing) = self.table.insert(
            hashed_value.hash,
            pointer,
            |a, b| a.hash == b.hash && a.data == b.data,
            |t| t.hash,
        );
        // Another thread may have interned the same value between the lookup and the insert;
        // `insert` then hands the value back and returns the entry that thread stored.
        let disposition = if existing.is_some() {
            InternDisposition::Interned
        } else {
            InternDisposition::Computed
        };
        (Intern { pointer }, disposition)
    }

    /// Get a value if it has been interned.
    pub fn get<Q>(&'static self, key: Q) -> Option<Intern<T>>
    where
        Q: Hash + Equivalent<T>,
        T: Eq + Hash,
    {
        let hashed = Hashed::<_, H>::new(key);
        self.table
            .lookup(hashed.hash, |t| hashed.value.equivalent(&t.data))
            .map(|pointer| Intern { pointer })
    }

    /// Iterate over the interned values.
    #[inline]
    pub fn iter(&'static self) -> Iter<T, H> {
        Iter {
            iter: self.table.iter(),
            _marker: PhantomData,
        }
    }
}

pub struct Iter<T: 'static, H: 'static> {
    iter: lock_free_hashtable::sharded::Iter<'static, Box<InternedData<T>>, 64>,
    _marker: PhantomData<H>,
}

impl<T: 'static, H: 'static> Iterator for Iter<T, H> {
    type Item = Intern<T>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|pointer| Intern { pointer })
    }
}

/// Define a static interner and implement Internable to associate it with an interned type.
///
/// Without additional arguments, `interner!(STRING_INTERNER, DefaultHasher, InternString)`
/// creates a `static STRING_INTERNER: Interner<InternString, DefaultHasher>` and implements Internable for InternString.
///
/// Additional arguments are used to implement convenience traits when the interned type wraps an existing type.
/// `interner!(STRING_INTERNER, DefaultHasher, InternString, String, str, StrRef)`
/// also implements
///     ``Equivalent<InternString>`` for `String`
///     ``From<String>``for `InternString`
///     `Deref<Target = str>` for `InternString`
/// and creates a new `StrRef<'a>(&'a str)` type that implements ``Equivalent<InternString>`` and ``From<StrRef>`` for InternString.
#[macro_export]
macro_rules! interner {
    ($interner_name:ident, $hasher:ty, $type:ty) => {
        static $interner_name: $crate::Interner<$type, $hasher> = $crate::Interner::new();

        impl $crate::Internable for $type {
            type Hasher = $hasher;

            fn interner() -> &'static $crate::Interner<Self, $hasher> {
                &$interner_name
            }
        }
    };
    ($interner_name:ident, $hasher:ty, $newtype:ident, $basetype:ty) => {
        impl $crate::Equivalent<$newtype> for $basetype {
            fn equivalent(&self, key: &$newtype) -> bool {
                self == &key.0
            }
        }

        impl From<$basetype> for $newtype {
            fn from(value: $basetype) -> Self {
                $newtype(value)
            }
        }

        $crate::interner!($interner_name, $hasher, $newtype);
    };
    ($interner_name:ident, $hasher:ty, $newtype:ident, $basetype:ty, $basereftype:ty) => {
        impl std::ops::Deref for $newtype {
            type Target = $basereftype;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        $crate::interner!($interner_name, $hasher, $newtype, $basetype);
    };
    ($interner_name:ident, $hasher:ty, $newtype:ident, $basetype:ty, $basereftype:ty, $newreftype:ident) => {
        #[derive(Hash, Eq, PartialEq)]
        pub struct $newreftype<'a>(&'a $basereftype);

        impl $crate::Equivalent<$newtype> for $newreftype<'_> {
            fn equivalent(&self, key: &$newtype) -> bool {
                self.0 == key.0
            }
        }

        impl From<$newreftype<'_>> for $newtype {
            fn from(value: $newreftype<'_>) -> Self {
                $newtype(value.0.into())
            }
        }

        $crate::interner!($interner_name, $hasher, $newtype, $basetype, $basereftype);
    };
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::collections::hash_map::DefaultHasher;

    use crate::Equivalent;
    use crate::Intern;
    use crate::InternDisposition;
    use crate::Interner;

    #[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
    pub struct StringValue(String);
    interner!(
        STRING_INTERNER,
        DefaultHasher,
        StringValue,
        String,
        str,
        StrRef
    );

    #[test]
    fn test_intern() {
        assert_eq!(
            STRING_INTERNER.intern("hello".to_owned()),
            STRING_INTERNER.intern("hello".to_owned())
        );
        assert_eq!(
            STRING_INTERNER.intern("hello".to_owned()),
            STRING_INTERNER.intern(StrRef("hello")),
        );
        assert_ne!(
            STRING_INTERNER.intern("hello".to_owned()),
            STRING_INTERNER.intern("world".to_owned())
        );
    }

    // Make sure things work with reallocation.
    #[test]
    fn test_resize() {
        let n = if cfg!(miri) { 2000 } else { 100000 };
        let mut interned_strings = Vec::new();
        for i in 0..n {
            let s = i.to_string();
            let interned = STRING_INTERNER.intern(s.clone());
            assert_eq!(&s, &*interned.0);
            interned_strings.push(interned);
        }

        for s in &interned_strings {
            let interned = STRING_INTERNER.intern(String::clone(&s.0));
            assert_eq!(*s, interned);
        }
    }

    #[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
    pub struct TestDispositionValue(String);
    interner!(
        TEST_DISPOSITION_INTERNER,
        DefaultHasher,
        TestDispositionValue,
        String
    );
    /// A lookup key whose conversion into the interned value waits until both racing threads
    /// have missed the lookup, which forces the schedule "A misses, B misses, A inserts, B finds
    /// A's entry".
    struct RacingKey(&'static str);

    static BOTH_MISSED: std::sync::LazyLock<std::sync::Barrier> =
        std::sync::LazyLock::new(|| std::sync::Barrier::new(2));

    #[derive(Debug, Hash, PartialEq, Eq)]
    struct RacedValue(String);

    static RACE_INTERNER: Interner<RacedValue> = Interner::new();

    impl std::hash::Hash for RacingKey {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            // The same as the derived `Hash` of `RacedValue`.
            self.0.to_owned().hash(state)
        }
    }

    impl Equivalent<RacedValue> for RacingKey {
        fn equivalent(&self, key: &RacedValue) -> bool {
            self.0 == key.0
        }
    }

    impl From<RacingKey> for RacedValue {
        fn from(k: RacingKey) -> RacedValue {
            BOTH_MISSED.wait();
            RacedValue(k.0.to_owned())
        }
    }

    /// Two threads that intern the same new value at once: the one whose insert finds the
    /// entry of the other reports `Interned`.
    #[test]
    fn test_disposition_of_racing_interns() {
        let (a, b) = std::thread::scope(|s| {
            let ta = s.spawn(|| RACE_INTERNER.observed_intern(RacingKey("new value")));
            let tb = s.spawn(|| RACE_INTERNER.observed_intern(RacingKey("new value")));
            (ta.join().unwrap(), tb.join().unwrap())
        });
        assert_eq!(a.0, b.0);
        assert_eq!(RACE_INTERNER.iter().count(), 1);
        let computed = [&a.1, &b.1]
            .iter()
            .filter(|d| matches!(d, InternDisposition::Computed))
            .count();
        assert_eq!(computed, 1);
    }

    #[test]
    fn test_disposition() {
        let (val, disposition) = TEST_DISPOSITION_INTERNER.observed_intern("hello".to_owned());
        assert_eq!(val.0.to_string(), "hello".to_owned());
        assert!(std::matches!(disposition, InternDisposition::Computed));

        let (val, disposition) = TEST_DISPOSITION_INTERNER.observed_intern("hello".to_owned());
        assert_eq!(val.0.to_string(), "hello".to_owned());
        assert!(std::matches!(disposition, InternDisposition::Interned));
    }

    #[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
    pub struct TestGetValue(String);
    interner!(
        TEST_GET_INTERNER,
        DefaultHasher,
        TestGetValue,
        String,
        str,
        TestGetRef
    );
    #[test]
    fn test_get() {
        let interner = &TEST_GET_INTERNER;
        assert_eq!(interner.get(TestGetRef("hello")), None);
        assert_eq!(interner.get("hello".to_owned()), None);

        let interned = interner.intern("hello".to_owned());
        assert_eq!(interner.get(TestGetRef("hello")), Some(interned));
        assert_eq!(interner.get("hello".to_owned()), Some(interned));
        assert_eq!(interner.get(TestGetRef("world")), None);
    }

    #[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
    pub struct TestIterValue(&'static str);
    interner!(
        TEST_ITER_INTERNER,
        DefaultHasher,
        TestIterValue,
        &'static str
    );
    #[test]
    fn test_iter() {
        let interner = &TEST_ITER_INTERNER;
        assert_eq!(
            interner
                .iter()
                .map(|v| v.0)
                .collect::<BTreeSet<&'static str>>(),
            BTreeSet::from([])
        );
        interner.intern("hello");
        interner.intern("cat");
        interner.intern("world");

        assert_eq!(
            interner
                .iter()
                .map(|v| v.0)
                .collect::<BTreeSet<&'static str>>(),
            BTreeSet::from(["hello", "cat", "world"])
        );
    }

    #[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
    pub struct TestPointerValue(&'static str);
    interner!(
        TEST_POINTER_INTERNER,
        DefaultHasher,
        TestPointerValue,
        &'static str
    );
    #[test]
    fn test_pointer_roundtrip() {
        let one = TEST_POINTER_INTERNER.intern("one");
        assert_eq!(one, unsafe { Intern::from_ptr(one.as_ptr()) });
    }
}
