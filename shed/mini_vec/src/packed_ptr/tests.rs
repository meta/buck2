/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

use crate::packed_ptr::PackedPtr;

#[cfg(not(mini_vec_no_ptr_packing))]
#[test]
fn packed_ptr_round_trips() {
    static TARGET: u32 = 7;
    let p = PackedPtr::new(&TARGET, 0xAB);
    let ptr = p.to_packed_ptr();
    // SAFETY: `ptr` came from `to_packed_ptr` on a `PackedPtr<&'static u32>`.
    let q: PackedPtr<&'static u32> = unsafe { PackedPtr::from_packed_ptr(ptr) };
    assert_eq!(q.extra(), 0xAB);
    assert!(std::ptr::eq(q.as_ref(), &TARGET));
    assert_eq!(q.to_packed_ptr(), ptr);
}

#[test]
fn size_is_one_pointer() {
    // Packing fits in a single machine word; with packing disabled the metadata
    // byte sits in its own field, padding the value to two words.
    #[cfg(not(mini_vec_no_ptr_packing))]
    const WORDS: usize = 1;
    #[cfg(mini_vec_no_ptr_packing)]
    const WORDS: usize = 2;
    let expected = WORDS * size_of::<*const u8>();
    assert_eq!(size_of::<PackedPtr<NonNull<u32>>>(), expected);
    assert_eq!(size_of::<PackedPtr<Box<u32>>>(), expected);
    assert_eq!(size_of::<PackedPtr<Arc<u32>>>(), expected);
    // Niche optimization: the non-null pointer's niche means `Option` adds no
    // size, in either representation.
    assert_eq!(
        size_of::<Option<PackedPtr<NonNull<u32>>>>(),
        size_of::<PackedPtr<NonNull<u32>>>(),
    );
}

#[test]
fn ref_round_trip_preserves_pointer_and_extra() {
    let value: u32 = 0xCAFEBABE;
    let nn = NonNull::from(&value);
    let p = PackedPtr::<NonNull<u32>>::new(nn, 0xAB);
    assert_eq!(p.extra(), 0xAB);
    assert_eq!(p.as_ref(), nn);

    let (recovered, extra) = p.into_parts();
    assert_eq!(extra, 0xAB);
    assert_eq!(recovered, nn);
}

#[test]
fn extra_zero_and_max_round_trip() {
    let value: u32 = 7;
    let nn = NonNull::from(&value);
    let p0 = PackedPtr::<NonNull<u32>>::new(nn, 0);
    assert_eq!(p0.extra(), 0);
    assert_eq!(p0.as_ref(), nn);

    let pmax = PackedPtr::<NonNull<u32>>::new(nn, u8::MAX);
    assert_eq!(pmax.extra(), u8::MAX);
    assert_eq!(pmax.as_ref(), nn);
}

#[test]
fn set_extra_does_not_disturb_pointer() {
    let value: u64 = 12345;
    let original = NonNull::from(&value);
    let mut p = PackedPtr::<NonNull<u64>>::new(original, 0);
    for e in [1u8, 0xFE, 0x0F, u8::MAX, 0] {
        p.set_extra(e);
        assert_eq!(p.extra(), e);
        assert_eq!(p.as_ref(), original);
    }
}

#[test]
fn box_round_trip_drops_inner() {
    // Verifies the `Drop` impl actually frees the box.
    let counter = Rc::new(Cell::new(0usize));

    struct DropCounter(Rc<Cell<usize>>);
    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    {
        let p = PackedPtr::<Box<DropCounter>>::new(Box::new(DropCounter(counter.clone())), 42);
        assert_eq!(p.extra(), 42);
        assert_eq!(counter.get(), 0);
    }
    // PackedPtr was dropped, which should drop the box, which drops the
    // counter exactly once.
    assert_eq!(counter.get(), 1);
}

#[test]
fn box_into_inner_recovers_box_without_dropping() {
    let counter = Rc::new(Cell::new(0usize));

    struct DropCounter(Rc<Cell<usize>>);
    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    let p = PackedPtr::<Box<DropCounter>>::new(Box::new(DropCounter(counter.clone())), 0);
    let b = p.into_inner();
    // Recovering must not have dropped the inner.
    assert_eq!(counter.get(), 0);
    drop(b);
    assert_eq!(counter.get(), 1);
}

#[test]
fn arc_round_trip_preserves_refcount() {
    let arc = Arc::new(99u32);
    assert_eq!(Arc::strong_count(&arc), 1);

    let p = PackedPtr::<Arc<u32>>::new(arc.clone(), 0xBE);
    assert_eq!(Arc::strong_count(&arc), 2);
    assert_eq!(*p.as_ref(), 99);
    assert_eq!(p.extra(), 0xBE);

    drop(p);
    // Drop must release the Arc clone we packed.
    assert_eq!(Arc::strong_count(&arc), 1);
}

#[test]
fn arc_clone_bumps_refcount_and_drops_cleanly() {
    let arc = Arc::new(7u32);
    assert_eq!(Arc::strong_count(&arc), 1);

    let p1 = PackedPtr::<Arc<u32>>::new(arc.clone(), 1);
    assert_eq!(Arc::strong_count(&arc), 2);

    let p2 = p1.clone();
    // Cloning the PackedPtr<Arc> must clone the underlying Arc (refcount +1).
    assert_eq!(Arc::strong_count(&arc), 3);
    assert_eq!(p2.extra(), 1);
    assert!(std::ptr::eq(p1.as_ref(), p2.as_ref()));

    drop(p1);
    assert_eq!(Arc::strong_count(&arc), 2);
    drop(p2);
    assert_eq!(Arc::strong_count(&arc), 1);
}

#[test]
fn box_clone_deep_copies() {
    let p = PackedPtr::<Box<String>>::new(Box::new("hello".to_owned()), 5);
    let p2 = p.clone();
    // Same value...
    assert_eq!(*p.as_ref(), "hello");
    assert_eq!(*p2.as_ref(), "hello");
    assert_eq!(p2.extra(), 5);
    // ...different allocations.
    assert!(!std::ptr::eq(p.as_ref(), p2.as_ref()));
}

#[test]
fn ref_clone_shares_pointer() {
    let value = 1234u32;
    let nn = NonNull::from(&value);
    let p = PackedPtr::<NonNull<u32>>::new(nn, 9);
    #[expect(
        clippy::clone_on_copy,
        reason = "explicitly testing that .clone() works via the Clone impl"
    )]
    let p2 = p.clone();
    assert_eq!(p.as_ref(), p2.as_ref());
    assert_eq!(p2.extra(), 9);
}

#[test]
fn eq_and_hash_compare_values_plus_extra() {
    let a = PackedPtr::<Box<u32>>::new(Box::new(42), 5);
    let b = PackedPtr::<Box<u32>>::new(Box::new(42), 5);
    let c = PackedPtr::<Box<u32>>::new(Box::new(42), 6);
    let d = PackedPtr::<Box<u32>>::new(Box::new(43), 5);

    assert_eq!(a, b, "equal values + equal extra → equal");
    assert_ne!(a, c, "equal values, different extra → unequal");
    assert_ne!(a, d, "different values, equal extra → unequal");

    let mut h1 = DefaultHasher::new();
    a.hash(&mut h1);
    let mut h2 = DefaultHasher::new();
    b.hash(&mut h2);
    assert_eq!(
        h1.finish(),
        h2.finish(),
        "equal PackedPtrs hash identically"
    );
}

#[test]
fn debug_prints_pointee_and_extra() {
    // Use `Box` so `Ref<'a> = &'a u32` and the pointee value prints.
    let p = PackedPtr::<Box<u32>>::new(Box::new(7), 100);
    let s = format!("{p:?}");
    assert!(s.contains("PackedPtr"), "got {s}");
    assert!(s.contains('7'), "got {s}");
    assert!(s.contains("100"), "got {s}");
}

static OPTION_TEST_VALUE: u32 = 0xCAFEBABE;

#[test]
fn option_size_is_one_pointer() {
    // Wrapping the pointer value in `Option` costs nothing: `None` is encoded as
    // the dangling sentinel, so `PackedPtr<Option<P>>` is the same size as
    // `PackedPtr<P>` — one word when packing, two without it — on both the
    // owning (`Box`/`Arc`) and `Copy` (`&'static`) sides.
    assert_eq!(
        size_of::<PackedPtr<Option<Box<u32>>>>(),
        size_of::<PackedPtr<Box<u32>>>(),
    );
    assert_eq!(
        size_of::<PackedPtr<Option<Arc<u32>>>>(),
        size_of::<PackedPtr<Arc<u32>>>(),
    );
    assert_eq!(
        size_of::<PackedPtr<Option<&'static u32>>>(),
        size_of::<PackedPtr<&'static u32>>(),
    );
}

#[test]
fn option_box_none_round_trip() {
    let p = PackedPtr::<Option<Box<u32>>>::new(None, 0xAB);
    assert_eq!(p.extra(), 0xAB);
    assert_eq!(p.as_ref(), None);
    let (recovered, extra) = p.into_parts();
    assert_eq!(extra, 0xAB);
    assert!(recovered.is_none(), "expected None");
}

#[test]
fn option_box_some_round_trip() {
    let p = PackedPtr::<Option<Box<u32>>>::new(Some(Box::new(42)), 0x12);
    assert_eq!(p.extra(), 0x12);
    assert_eq!(p.as_ref(), Some(&42));
    let (recovered, extra) = p.into_parts();
    assert_eq!(extra, 0x12);
    assert_eq!(recovered.as_deref(), Some(&42));
}

#[test]
fn option_box_some_drops_inner() {
    // `PackedPtr<Option<Box<T>>>` owns its `Some` payload — dropping it
    // must run the `Box`'s `Drop`.
    let counter = Rc::new(Cell::new(0usize));
    struct DropCounter(Rc<Cell<usize>>);
    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    {
        let _p = PackedPtr::<Option<Box<DropCounter>>>::new(
            Some(Box::new(DropCounter(counter.clone()))),
            0,
        );
        assert_eq!(counter.get(), 0);
    }
    assert_eq!(
        counter.get(),
        1,
        "dropping `PackedPtr<Option<Box<_>>>` must drop the `Box` inside `Some`",
    );
}

#[test]
fn option_box_none_drop_is_noop() {
    // Dropping a `None` packed pointer must not touch (or attempt to free)
    // the dangling sentinel address — just verify no panic / no UB under
    // miri.
    let _p = PackedPtr::<Option<Box<u32>>>::new(None, 7);
}

#[test]
fn option_static_ref_none_round_trip() {
    let p = PackedPtr::<Option<&'static u32>>::new(None, 5);
    assert_eq!(p.extra(), 5);
    assert_eq!(p.as_ref(), None);
    let (recovered, extra) = p.into_parts();
    assert_eq!(extra, 5);
    assert!(recovered.is_none());
}

#[test]
fn option_static_ref_some_round_trip() {
    let p = PackedPtr::<Option<&'static u32>>::new(Some(&OPTION_TEST_VALUE), 0xBE);
    assert_eq!(p.extra(), 0xBE);
    assert_eq!(p.as_ref(), Some(&0xCAFEBABE));
    let (recovered, extra) = p.into_parts();
    assert_eq!(extra, 0xBE);
    assert_eq!(recovered, Some(&OPTION_TEST_VALUE));
}

#[test]
fn option_static_ref_is_copy() {
    // For `Copy` `X`, `PackedPtr<Option<X>>` must itself be `Copy`.
    fn assert_copy<T: Copy>() {}
    assert_copy::<PackedPtr<Option<&'static u32>>>();

    let p = PackedPtr::<Option<&'static u32>>::new(Some(&OPTION_TEST_VALUE), 3);
    let p_copy = p;
    // Using `p` again after the move would fail to compile if `Copy` were
    // not implemented.
    assert_eq!(p.extra(), 3);
    assert_eq!(p_copy.extra(), 3);
    assert_eq!(p.as_ref(), Some(&0xCAFEBABE));
    assert_eq!(p_copy.as_ref(), Some(&0xCAFEBABE));
}

#[test]
fn into_parts_does_not_double_drop() {
    // Roundtrip: PackedPtr<Box<T>> → into_parts → drop the recovered Box.
    // The drop count must be exactly one.
    let counter = Rc::new(Cell::new(0usize));
    struct DropCounter(Rc<Cell<usize>>);
    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    let p = PackedPtr::<Box<DropCounter>>::new(Box::new(DropCounter(counter.clone())), 0xF0);
    let (b, extra) = p.into_parts();
    assert_eq!(extra, 0xF0);
    assert_eq!(counter.get(), 0);
    drop(b);
    assert_eq!(counter.get(), 1);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::packed_ptr::PackedPtr;

    /// Total heap+stack size allocative attributes to `root`, asserting along the
    /// way that the flame graph reports no size inconsistencies (which would mean a
    /// node declared a size smaller than its inline children).
    fn flame_total(root: &dyn allocative::Allocative) -> usize {
        let mut builder = allocative::FlameGraphBuilder::default();
        builder.visit_root(root);
        let out = builder.finish();
        assert_eq!(out.warnings(), "", "unexpected flame graph size warnings");
        out.flamegraph().total_size()
    }

    #[test]
    fn allocative_smoke() {
        static SMOKE_STATIC: u64 = 7;

        // Exercise the visitor across every `Allocative` pointer kind. A mismatched
        // `Visitor::enter_*` / `exit` would panic, and a bad size declaration would
        // surface as a flame graph warning (asserted away inside `flame_total`).
        // `NonNull` is intentionally not `Allocative`, so `PackedPtr<NonNull<_>>`
        // isn't either and cannot be visited here.
        assert_eq!(flame_total(&PackedPtr::<Box<u32>>::new(Box::new(5), 2)), 12);
        assert_eq!(flame_total(&PackedPtr::<Arc<u32>>::new(Arc::new(5), 3)), 32);
        assert_eq!(
            flame_total(&PackedPtr::<&'static u64>::new(&SMOKE_STATIC, 4)),
            8
        );
        assert_eq!(
            flame_total(&PackedPtr::<Option<Box<u32>>>::new(Some(Box::new(5)), 5)),
            12
        );
        assert_eq!(flame_total(&PackedPtr::<Option<Box<u32>>>::new(None, 6)), 8);
    }
}
