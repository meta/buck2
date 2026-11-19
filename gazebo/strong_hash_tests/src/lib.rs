/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![cfg(test)]

use std::hash::Hasher;

use strong_hash::StrongHash;

#[test]
fn test_strong_hash_derive() {
    // First test that the derive works.
    #[derive(Clone, StrongHash)]
    struct Foo(u8, String, Vec<u8>);

    #[derive(Clone, StrongHash)]
    struct Bar {
        foo: Foo,
        size: usize,
    }

    #[derive(StrongHash)]
    enum FooBar {
        EnumFoo(Foo),
        EnumBar { bar: Bar, size: usize },
    }

    // Now test that these different datatypes produce different hashes

    // First actually implement a strong hasher
    pub struct Blake3StrongHasher(blake3::Hasher);

    impl Blake3StrongHasher {
        pub fn new() -> Self {
            Self(blake3::Hasher::new())
        }

        pub fn digest(&self) -> blake3::Hash {
            self.0.finalize()
        }
    }

    // This `Hasher` impl only provides `write` and `finish` (not the full
    // set of `write_*` methods). That is fine for blake3 since it is a
    // streaming hash. See the comment on `StarlarkHasherSmallPromote` in
    // `buck2_build_api/.../provider/callable.rs` for the full story.
    impl Hasher for Blake3StrongHasher {
        fn write(&mut self, bytes: &[u8]) {
            self.0.update(bytes);
        }

        fn finish(&self) -> u64 {
            let bytes = self.digest().as_bytes()[..8]
                .try_into()
                .expect("Internal error: hash should be 64 bits");
            u64::from_be_bytes(bytes)
        }
    }

    fn hash(hashable: &impl StrongHash) -> u64 {
        let mut hasher = Blake3StrongHasher::new();
        hashable.strong_hash(&mut hasher);
        Hasher::finish(&hasher)
    }

    // Now test that hashes are not equal
    let foo = Foo(1, "hello".to_owned(), vec![]);
    let bar = Bar {
        foo: foo.clone(),
        size: 1usize,
    };
    let foo_bar_foo = FooBar::EnumFoo(foo.clone());
    let foo_bar_bar = FooBar::EnumBar {
        bar: bar.clone(),
        size: 1usize,
    };

    let foo_hash = hash(&foo);
    let bar_hash = hash(&bar);
    let foo_bar_foo_hash = hash(&foo_bar_foo);
    let foo_bar_bar_hash = hash(&foo_bar_bar);

    assert_ne!(foo_hash, bar_hash);
    assert_ne!(foo_hash, foo_bar_foo_hash);
    assert_ne!(foo_hash, foo_bar_bar_hash);
    assert_ne!(bar_hash, foo_bar_foo_hash);
    assert_ne!(bar_hash, foo_bar_bar_hash);
    assert_ne!(foo_bar_foo_hash, foo_bar_bar_hash);
}

#[test]
fn test_generics() {
    #[derive(StrongHash)]
    struct Foo<T>(T);

    fn check_is_implemented<T: StrongHash>(_t: &T) {}

    let foo = Foo(1u8);
    check_is_implemented(&foo);
}
