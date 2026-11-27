/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A cheap version of [`Clone`].
//!
//! # Features
//!
//! - `alloc`: [`Dupe`] impls for `Arc`, `Rc` and their `Weak` pointers.
//! - `derive` (default): re-exports the derive macros from `dupe_derive`.
//! - `std` (default): implies `alloc`; adds impls for `std`-only types
//!   such as `Instant`, `SystemTime` and `ThreadId`.
//!
//! With `default-features = false` this crate is `no_std`.

#![no_std]

#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

pub mod __macro_refs;
pub(crate) mod iter;
pub(crate) mod option;
pub(crate) mod result;

use core::cell::Cell;
use core::mem::ManuallyDrop;
use core::num::*;

#[cfg(feature = "derive")]
pub use dupe_derive::Clone_;
#[cfg(feature = "derive")]
pub use dupe_derive::Copy_;
#[cfg(feature = "derive")]
pub use dupe_derive::Dupe;
#[cfg(feature = "derive")]
pub use dupe_derive::Dupe_;

pub use crate::iter::IterDupedExt;
pub use crate::option::OptionDupedExt;
pub use crate::result::ResultDupedErrExt;
pub use crate::result::ResultDupedExt;

/// Like [`Clone`], but should only be available if [`Clone`] is
/// constant time and zero allocation (e.g. a few
/// [`Arc`](https://doc.rust-lang.org/alloc/sync/struct.Arc.html) bumps).
/// The implementation of `dupe` should _always_ call `clone`.
pub trait Dupe: Clone {
    #[inline]
    fn dupe(&self) -> Self {
        self.clone()
    }
}

// Smart pointer/wrapper types
impl<A: ?Sized> Dupe for &A {}
impl<A: ?Sized> Dupe for *const A {}
impl<A: ?Sized> Dupe for *mut A {}
#[cfg(all(feature = "alloc", target_has_atomic = "ptr"))]
impl<A: ?Sized> Dupe for alloc::sync::Arc<A> {}
#[cfg(all(feature = "alloc", target_has_atomic = "ptr"))]
impl<A: ?Sized> Dupe for alloc::sync::Weak<A> {}
#[cfg(feature = "alloc")]
impl<A: ?Sized> Dupe for alloc::rc::Rc<A> {}
#[cfg(feature = "alloc")]
impl<A: ?Sized> Dupe for alloc::rc::Weak<A> {}
impl<A: Copy> Dupe for Cell<A> {}
impl<A: Dupe> Dupe for ManuallyDrop<A> {}

// Small containers
impl<A: Dupe> Dupe for Option<A> {}
impl<T: Dupe, E: Dupe> Dupe for Result<T, E> {}
impl<A: Dupe> Dupe for core::ops::Bound<A> {}
impl<A: Dupe> Dupe for core::pin::Pin<A> {}
impl<A: Dupe> Dupe for core::ptr::NonNull<A> {}
impl<A: Dupe> Dupe for core::task::Poll<A> {}
impl Dupe for () {}
impl<A: Dupe> Dupe for (A,) {}
impl<A: Dupe, B: Dupe> Dupe for (A, B) {}
impl<A: Dupe, B: Dupe, C: Dupe> Dupe for (A, B, C) {}
impl<A: Dupe, B: Dupe, C: Dupe, D: Dupe> Dupe for (A, B, C, D) {}
impl<A: Dupe, B: Dupe, C: Dupe, D: Dupe, E: Dupe> Dupe for (A, B, C, D, E) {}
impl<A: Dupe, B: Dupe, C: Dupe, D: Dupe, E: Dupe, F: Dupe> Dupe for (A, B, C, D, E, F) {}
impl<A: Dupe, B: Dupe, C: Dupe, D: Dupe, E: Dupe, F: Dupe, G: Dupe> Dupe for (A, B, C, D, E, F, G) {}
impl<A: Dupe, B: Dupe, C: Dupe, D: Dupe, E: Dupe, F: Dupe, G: Dupe, H: Dupe> Dupe
    for (A, B, C, D, E, F, G, H)
{
}
impl<A: Dupe, B: Dupe, C: Dupe, D: Dupe, E: Dupe, F: Dupe, G: Dupe, H: Dupe, I: Dupe> Dupe
    for (A, B, C, D, E, F, G, H, I)
{
}
impl<A: Dupe, B: Dupe, C: Dupe, D: Dupe, E: Dupe, F: Dupe, G: Dupe, H: Dupe, I: Dupe, J: Dupe> Dupe
    for (A, B, C, D, E, F, G, H, I, J)
{
}

impl<A: Dupe, const N: usize> Dupe for [A; N] {}

// Atomic types
impl Dupe for bool {}
impl Dupe for char {}
impl Dupe for u8 {}
impl Dupe for u16 {}
impl Dupe for u32 {}
impl Dupe for u64 {}
impl Dupe for u128 {}
impl Dupe for usize {}
impl Dupe for i8 {}
impl Dupe for i16 {}
impl Dupe for i32 {}
impl Dupe for i64 {}
impl Dupe for i128 {}
impl Dupe for isize {}
impl Dupe for f32 {}
impl Dupe for f64 {}
impl Dupe for NonZeroU8 {}
impl Dupe for NonZeroU16 {}
impl Dupe for NonZeroU32 {}
impl Dupe for NonZeroU64 {}
impl Dupe for NonZeroU128 {}
impl Dupe for NonZeroUsize {}
impl Dupe for NonZeroI8 {}
impl Dupe for NonZeroI16 {}
impl Dupe for NonZeroI32 {}
impl Dupe for NonZeroI64 {}
impl Dupe for NonZeroI128 {}
impl Dupe for NonZeroIsize {}

// Other core types that are Copyable
impl Dupe for core::any::TypeId {}
impl Dupe for core::marker::PhantomPinned {}
impl Dupe for core::net::Ipv4Addr {}
impl Dupe for core::net::Ipv6Addr {}
impl Dupe for core::net::SocketAddrV4 {}
impl Dupe for core::net::SocketAddrV6 {}
impl Dupe for core::time::Duration {}
impl<T: ?Sized> Dupe for core::marker::PhantomData<T> {}

// Other std types that are Copyable
#[cfg(feature = "std")]
impl Dupe for std::thread::ThreadId {}
#[cfg(feature = "std")]
impl Dupe for std::time::Instant {}
#[cfg(feature = "std")]
impl Dupe for std::time::SystemTime {}

impl<R> Dupe for fn() -> R {}
impl<A1, R> Dupe for fn(A1) -> R {}
impl<A1, A2, R> Dupe for fn(A1, A2) -> R {}
impl<A1, A2, A3, R> Dupe for fn(A1, A2, A3) -> R {}
impl<A1, A2, A3, A4, R> Dupe for fn(A1, A2, A3, A4) -> R {}
impl<A1, A2, A3, A4, A5, R> Dupe for fn(A1, A2, A3, A4, A5) -> R {}
impl<A1, A2, A3, A4, A5, A6, R> Dupe for fn(A1, A2, A3, A4, A5, A6) -> R {}
impl<A1, A2, A3, A4, A5, A6, A7, R> Dupe for fn(A1, A2, A3, A4, A5, A6, A7) -> R {}
impl<A1, A2, A3, A4, A5, A6, A7, A8, R> Dupe for fn(A1, A2, A3, A4, A5, A6, A7, A8) -> R {}
impl<A1, A2, A3, A4, A5, A6, A7, A8, A9, R> Dupe for fn(A1, A2, A3, A4, A5, A6, A7, A8, A9) -> R {}
impl<A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, R> Dupe
    for fn(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10) -> R
{
}
impl<A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11, R> Dupe
    for fn(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11) -> R
{
}
impl<A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11, A12, R> Dupe
    for fn(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11, A12) -> R
{
}
// Rust goes up to 12 arguments for traits, so we follow

#[cfg(all(test, any(feature = "alloc", feature = "derive")))]
mod tests {
    use super::*;
    // Not actually unused, this makes testing the derive macro work
    #[cfg(feature = "derive")]
    #[allow(unused_imports)]
    use crate as dupe;

    #[cfg(feature = "derive")]
    #[test]
    fn test_dupe_generic() {
        #[derive(Clone, Dupe, Debug, PartialEq, Eq)]
        struct Foo {}
        #[derive(Clone, Dupe, Debug, PartialEq, Eq)]
        struct FooT<T> {
            foo: T,
        }
        #[derive(Clone, Dupe, Debug, PartialEq, Eq)]
        struct Faz;

        let x = Foo {};
        assert_eq!(x, Dupe::dupe(&x));

        let x = FooT { foo: 1 };
        assert_eq!(x, Dupe::dupe(&x));

        let x = Faz;
        assert_eq!(x, Dupe::dupe(&x));
    }

    #[cfg(all(feature = "alloc", feature = "derive"))]
    #[test]
    fn test_dupe_() {
        use alloc::sync::Arc;

        #[derive(Debug, PartialEq, Eq)]
        struct NoClone();
        #[derive(Dupe_, Debug, PartialEq, Eq)]
        struct FooT<T> {
            foo: Arc<T>,
        }

        impl<T> Clone for FooT<T> {
            fn clone(&self) -> Self {
                FooT {
                    foo: self.foo.clone(),
                }
            }
        }

        let x = FooT {
            foo: Arc::new(NoClone()),
        };
        assert_eq!(x, Dupe::dupe(&x));
    }

    #[cfg(feature = "derive")]
    #[test]
    fn test_dupe_enum() {
        #[derive(Clone, Dupe, Debug, PartialEq, Eq)]
        struct Foo();
        #[derive(Clone, Dupe, Debug, PartialEq, Eq)]
        struct Foo2;
        #[derive(Clone, Dupe, Debug, PartialEq, Eq)]
        struct Bar(i64, bool, Foo2);
        #[derive(Clone, Dupe, Debug, PartialEq, Eq)]
        struct Baz {
            foo: usize,
        }
        #[derive(Clone, Dupe, Debug, PartialEq, Eq)]
        enum Qux {
            Foo(),
            Foo2,
            Bar(Foo, Bar),
            Baz { foo: Foo, bar: Bar, baz: Baz },
        }

        let x = Qux::Bar(Foo(), Bar(8, true, Foo2));
        assert_eq!(x, Dupe::dupe(&x));
        let x = Qux::Baz {
            foo: Foo(),
            bar: Bar(7, false, Foo2),
            baz: Baz { foo: 9 },
        };
        assert_eq!(x, Dupe::dupe(&x));
        let x = Qux::Foo();
        assert_eq!(x, Dupe::dupe(&x));
        let x = Qux::Foo2;
        assert_eq!(x, Dupe::dupe(&x));
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn test_dupe_fn()
    where
        fn(usize): Dupe,
        fn(alloc::string::String, alloc::vec::Vec<usize>) -> bool: Dupe,
    {
        // Tests are in the where
    }
}
