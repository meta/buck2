/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::any::Any;
use std::any::TypeId;
use std::marker::PhantomData;

/// A comparable "token" that can be returned to wrap a reference to an [`Any`
/// type](Any) for [`PartialEq`](PartialEq).
///
/// This lets dyn traits be comparable by having all implementations return some
/// "token" that can be considered [`PartialEq`](PartialEq).
pub struct PartialEqAny<'a> {
    type_id: TypeId,
    cmp: unsafe fn(*const (), *const ()) -> bool,
    val: *const (),
    _marker: PhantomData<&'a dyn Any>,
}

impl<'a> PartialEqAny<'a> {
    #[inline]
    pub fn new<A: PartialEq + 'static>(a: &'a A) -> Self {
        PartialEqAny {
            type_id: TypeId::of::<A>(),
            cmp: |this, other| {
                // SAFETY: We only call `cmp` with
                //   `this.type_id == other.type_id == TypeId::of::<A>()`.
                let this = unsafe { &*(this as *const A) };
                let other = unsafe { &*(other as *const A) };
                this == other
            },
            val: a as *const A as *const (),
            _marker: PhantomData,
        }
    }

    /// Get `TypeId` of the referenced type.
    #[inline]
    pub fn type_id(&self) -> TypeId {
        self.type_id
    }

    /// gets an instance that always compares to false
    #[inline]
    pub fn always_false() -> Self {
        struct AlwaysFalse;

        impl PartialEq for AlwaysFalse {
            fn eq(&self, _other: &Self) -> bool {
                false
            }
        }

        PartialEqAny::new(&AlwaysFalse)
    }
}

impl<'a> PartialEq for PartialEqAny<'a> {
    #[inline]
    fn eq(&self, other: &PartialEqAny<'a>) -> bool {
        self.type_id == other.type_id && unsafe { (self.cmp)(self.val, other.val) }
    }
}

#[cfg(test)]
#[allow(clippy::bool_assert_comparison)]
mod tests {
    use crate::eq::PartialEqAny;

    struct Wrap<T>(T);

    impl<T: PartialEq + 'static> Wrap<T> {
        fn token(&self) -> PartialEqAny<'_> {
            PartialEqAny::new(&self.0)
        }
    }

    #[test]
    fn test_cmp_any() {
        let w1 = Wrap(1);
        let w2 = Wrap(1);
        let w3 = Wrap(2);

        assert!(w1.token() == w2.token());
        assert!(w1.token() != w3.token());

        let w4 = Wrap("foo");
        let w5 = Wrap("foo");
        let w6 = Wrap("bar");

        assert!(w4.token() == w5.token());
        assert!(w4.token() != w6.token());

        assert!(w1.token() != w6.token());
    }

    #[test]
    #[allow(clippy::eq_op)]
    fn always_false_cmp() {
        let w = Wrap(1);
        let f = PartialEqAny::always_false();

        assert!(f != f);
        assert!(f != w.token());
    }
}
