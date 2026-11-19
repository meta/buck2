/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Provides a number of size assertion macros.
//!
//! These macros are similar to ones already available in various ecosystem crates, but - aside from
//! some light API adjustments - are distinguished in that they are automatically disabled when this
//! crate's pointer packing is disabled, ensuring that it doesn't introduce compilation failures.

// The macros below expand to a `const _` item, so they can be invoked at module level. Do not wrap
// the expansion in a block `{ ... }` - that turns it into a block expression that is only valid in
// statement position.
//
// Each macro is spelled as a `__`-prefixed `macro_rules!` definition plus a re-export under its
// real name: `#[macro_export]` can only put a macro at the crate root, and the re-export is what
// makes it reachable as `size_assert::<name>`.

/// Assert that the provided type has size equal to the specified number of pointers.
pub use crate::__words_of_type as words_of_type;

#[cfg(all(not(mini_vec_no_ptr_packing), target_pointer_width = "64"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __words_of_type {
    ($ty:ty, $w:literal) => {
        const _: () = {
            const ACTUAL_BYTES: usize = ::core::mem::size_of::<$ty>();
            const EXPECTED_BYTES: usize = ::core::mem::size_of::<[usize; $w]>();

            let _ = <$ty as $crate::size_assert::__macro_refs::TypeHasExpectedWordSize<
                ACTUAL_BYTES,
                { ACTUAL_BYTES / ::core::mem::size_of::<usize>() },
                EXPECTED_BYTES,
                $w,
            >>::ASSERT;
        };
    };
}

// Note that it's very intentional that the `cfg` is outside the macro, not inside it. That way it
// only needs to be set when building this crate, not whichever crate uses this one.

#[cfg(not(all(not(mini_vec_no_ptr_packing), target_pointer_width = "64")))]
#[doc(hidden)]
#[macro_export]
macro_rules! __words_of_type {
    ($($t:tt)*) => {};
}

/// Assert that the two provided types have equal size.
///
/// Prefer [`words_of_type`] when asserting against a fixed number of pointers; reach for this when
/// the point is that one type is layout-compatible with another (e.g. a newtype around `Arc<str>`).
pub use crate::__same_size as same_size;

#[cfg(all(not(mini_vec_no_ptr_packing), target_pointer_width = "64"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __same_size {
    ($a:ty, $b:ty) => {
        const _: () = {
            let _ = <$a as $crate::size_assert::__macro_refs::SameSizeAs<
                $b,
                { ::core::mem::size_of::<$a>() },
                { ::core::mem::size_of::<$b>() },
            >>::ASSERT;
        };
    };
}

#[cfg(not(all(not(mini_vec_no_ptr_packing), target_pointer_width = "64")))]
#[doc(hidden)]
#[macro_export]
macro_rules! __same_size {
    ($($t:tt)*) => {};
}

/// Assert that the future returned by the given async fn has size equal to the specified number of
/// pointers.
///
/// Writing the size as `~N` instead asserts it only approximately, allowing the actual size to
/// drift up to 10% (but at least 5 words) in each direction before the assertion fails. Prefer this
/// form for large futures, where the point is to catch substantial regressions and an exact
/// assertion would demand an update for every incidental size change.
///
/// Each argument should be written as `_`, which expands to a `panic!()` placeholder of the right
/// type. When that placeholder is not enough to determine the future's type - e.g. the function is
/// generic over a closure type that must be inferred from the argument - write that argument as an
/// explicit expression instead:
///
/// ```ignore
/// size_assert::words_of_async_fn_future!(my_async_fn, (_, |x| panic!(), _), 5);
/// ```
///
/// Like the `_` placeholders, explicit arguments are only ever type-checked, never run, so they may
/// themselves be (or contain) `panic!()`.
pub use crate::__words_of_async_fn_future as words_of_async_fn_future;

#[cfg(all(not(mini_vec_no_ptr_packing), target_pointer_width = "64"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __words_of_async_fn_future {
    ($f:path, ($($arg:tt)*), $w:literal) => {
        const _: () = {
            // The body is only ever type-checked, never run: the placeholders construct a value of
            // the future's type without needing real arguments, and the size check fires at compile
            // time regardless of the function being called.
            #[allow(unused, unreachable_code, clippy::diverging_sub_expression)]
            fn assert() {
                $crate::size_assert::__macro_refs::assert_async_fn_future_size!(
                    ($f) ($w) () $($arg)*
                );
            }
        };
    };
    ($f:path, ($($arg:tt)*), ~ $w:literal) => {
        const _: () = {
            #[allow(unused, unreachable_code, clippy::diverging_sub_expression)]
            fn assert() {
                $crate::size_assert::__macro_refs::assert_async_fn_future_size!(
                    ($f) (~ $w) () $($arg)*
                );
            }
        };
    };
}

#[cfg(not(all(not(mini_vec_no_ptr_packing), target_pointer_width = "64")))]
#[doc(hidden)]
#[macro_export]
macro_rules! __words_of_async_fn_future {
    ($($t:tt)*) => {};
}

/// Assert that the value of the given expression has size equal to the specified number of
/// pointers.
///
/// This is the escape hatch for values whose types cannot be named, notably closures and their
/// futures. The expression is only ever type-checked, never run, so it may contain `panic!()`
/// wherever a value is needed:
///
/// ```ignore
/// size_assert::words_of_expr!((async |x: u8| x).async_call_once((panic!(),)), 2);
/// ```
///
/// Supports the same approximate `~N` size spelling as [`words_of_async_fn_future`].
pub use crate::__words_of_expr as words_of_expr;

#[cfg(all(not(mini_vec_no_ptr_packing), target_pointer_width = "64"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __words_of_expr {
    ($e:expr, $w:literal) => {
        const _: () = {
            #[allow(unused, unreachable_code, clippy::diverging_sub_expression)]
            fn assert() {
                const ACTUAL_BYTES: usize =
                    $crate::size_assert::__macro_refs::size_of_return(|| $e);
                const EXPECTED_BYTES: usize = ::core::mem::size_of::<[usize; $w]>();

                $crate::size_assert::__macro_refs::ExprHasExpectedWordSize::<
                    ACTUAL_BYTES,
                    { ACTUAL_BYTES / ::core::mem::size_of::<usize>() },
                    EXPECTED_BYTES,
                    $w,
                >::assert_size(|| $e);
            }
        };
    };
    ($e:expr, ~ $w:literal) => {
        const _: () = {
            #[allow(unused, unreachable_code, clippy::diverging_sub_expression)]
            fn assert() {
                const ACTUAL_BYTES: usize =
                    $crate::size_assert::__macro_refs::size_of_return(|| $e);
                const ACTUAL_WORDS: usize = ACTUAL_BYTES / ::core::mem::size_of::<usize>();
                const BOUNDS: (usize, usize) =
                    $crate::size_assert::__macro_refs::approx_size_bounds($w);

                $crate::size_assert::__macro_refs::ExprHasApproxWordSize::<
                    ACTUAL_BYTES,
                    ACTUAL_WORDS,
                    $w,
                    { BOUNDS.0 },
                    { BOUNDS.1 },
                    { BOUNDS.0 <= ACTUAL_WORDS && ACTUAL_WORDS <= BOUNDS.1 },
                >::assert_size(|| $e);
            }
        };
    };
}

#[cfg(not(all(not(mini_vec_no_ptr_packing), target_pointer_width = "64")))]
#[doc(hidden)]
#[macro_export]
macro_rules! __words_of_expr {
    ($($t:tt)*) => {};
}

#[allow(missing_docs)]
#[doc(hidden)]
pub mod __macro_refs {
    // The function pointer infers an unnameable return type without evaluating its body.
    pub const fn size_of_return<T>(_: fn() -> T) -> usize {
        ::core::mem::size_of::<T>()
    }

    // If these change, update the doc comment on [`super::words_of_async_fn_future`].
    const APPROX_TOLERANCE_PERCENT: usize = 10;
    const APPROX_TOLERANCE_MIN_WORDS: usize = 5;

    /// The `(min, max)` word counts accepted by an approximate size assertion against `expected`.
    pub const fn approx_size_bounds(expected: usize) -> (usize, usize) {
        let mut tolerance = expected * APPROX_TOLERANCE_PERCENT / 100;
        if tolerance < APPROX_TOLERANCE_MIN_WORDS {
            tolerance = APPROX_TOLERANCE_MIN_WORDS;
        }
        (expected.saturating_sub(tolerance), expected + tolerance)
    }

    #[diagnostic::on_unimplemented(
        message = "Type `{Self}` has word count {ACTUAL_WORDS} ({ACTUAL_BYTES} bytes); expected {EXPECTED_WORDS} ({EXPECTED_BYTES} bytes)"
    )]
    pub trait TypeHasExpectedWordSize<
        const ACTUAL_BYTES: usize,
        const ACTUAL_WORDS: usize,
        const EXPECTED_BYTES: usize,
        const EXPECTED_WORDS: usize,
    >
    {
        const ASSERT: ();
    }

    impl<T, const BYTES: usize, const WORDS: usize>
        TypeHasExpectedWordSize<BYTES, WORDS, BYTES, WORDS> for T
    {
        const ASSERT: () = ();
    }

    #[diagnostic::on_unimplemented(
        message = "Expression of type `{Self}` has word count {ACTUAL_WORDS} ({ACTUAL_BYTES} bytes); expected {EXPECTED_WORDS} ({EXPECTED_BYTES} bytes)"
    )]
    pub trait ExprHasExpectedWordSize<
        const ACTUAL_BYTES: usize,
        const ACTUAL_WORDS: usize,
        const EXPECTED_BYTES: usize,
        const EXPECTED_WORDS: usize,
    >: Sized
    {
        fn assert_size(_: fn() -> Self);
    }

    impl<T, const BYTES: usize, const WORDS: usize>
        ExprHasExpectedWordSize<BYTES, WORDS, BYTES, WORDS> for T
    {
        fn assert_size(_: fn() -> Self) {}
    }

    #[diagnostic::on_unimplemented(
        message = "Future of type `{Self}` has word count {ACTUAL_WORDS} ({ACTUAL_BYTES} bytes); expected {EXPECTED_WORDS} ({EXPECTED_BYTES} bytes)"
    )]
    pub trait FutureHasExpectedWordSize<
        const ACTUAL_BYTES: usize,
        const ACTUAL_WORDS: usize,
        const EXPECTED_BYTES: usize,
        const EXPECTED_WORDS: usize,
    >: Sized
    {
        fn assert_size(_: fn() -> Self);
    }

    impl<T, const BYTES: usize, const WORDS: usize>
        FutureHasExpectedWordSize<BYTES, WORDS, BYTES, WORDS> for T
    {
        fn assert_size(_: fn() -> Self) {}
    }

    #[diagnostic::on_unimplemented(
        message = "Expression of type `{Self}` has word count {ACTUAL_WORDS} ({ACTUAL_BYTES} bytes), outside the range {MIN_WORDS}..={MAX_WORDS} accepted by the assertion of ~{EXPECTED_WORDS}; if the new size is expected, update the assertion to ~{ACTUAL_WORDS}"
    )]
    pub trait ExprHasApproxWordSize<
        const ACTUAL_BYTES: usize,
        const ACTUAL_WORDS: usize,
        const EXPECTED_WORDS: usize,
        const MIN_WORDS: usize,
        const MAX_WORDS: usize,
        const WITHIN_BOUNDS: bool,
    >: Sized
    {
        fn assert_size(_: fn() -> Self);
    }

    impl<
        T,
        const ACTUAL_BYTES: usize,
        const ACTUAL_WORDS: usize,
        const EXPECTED_WORDS: usize,
        const MIN_WORDS: usize,
        const MAX_WORDS: usize,
    >
        ExprHasApproxWordSize<
            ACTUAL_BYTES,
            ACTUAL_WORDS,
            EXPECTED_WORDS,
            MIN_WORDS,
            MAX_WORDS,
            true,
        > for T
    {
        fn assert_size(_: fn() -> Self) {}
    }

    #[diagnostic::on_unimplemented(
        message = "Future of type `{Self}` has word count {ACTUAL_WORDS} ({ACTUAL_BYTES} bytes), outside the range {MIN_WORDS}..={MAX_WORDS} accepted by the assertion of ~{EXPECTED_WORDS}; if the new size is expected, update the assertion to ~{ACTUAL_WORDS}"
    )]
    pub trait FutureHasApproxWordSize<
        const ACTUAL_BYTES: usize,
        const ACTUAL_WORDS: usize,
        const EXPECTED_WORDS: usize,
        const MIN_WORDS: usize,
        const MAX_WORDS: usize,
        const WITHIN_BOUNDS: bool,
    >: Sized
    {
        fn assert_size(_: fn() -> Self);
    }

    impl<
        T,
        const ACTUAL_BYTES: usize,
        const ACTUAL_WORDS: usize,
        const EXPECTED_WORDS: usize,
        const MIN_WORDS: usize,
        const MAX_WORDS: usize,
    >
        FutureHasApproxWordSize<
            ACTUAL_BYTES,
            ACTUAL_WORDS,
            EXPECTED_WORDS,
            MIN_WORDS,
            MAX_WORDS,
            true,
        > for T
    {
        fn assert_size(_: fn() -> Self) {}
    }

    #[diagnostic::on_unimplemented(
        message = "Size mismatch between `{Self}` ({LEFT_BYTES} bytes) and `{Rhs}` ({RIGHT_BYTES} bytes)"
    )]
    pub trait SameSizeAs<Rhs, const LEFT_BYTES: usize, const RIGHT_BYTES: usize> {
        const ASSERT: ();
    }

    impl<Lhs, Rhs, const BYTES: usize> SameSizeAs<Rhs, BYTES, BYTES> for Lhs {
        const ASSERT: () = ();
    }

    /// Emits the size assertion for [`super::words_of_async_fn_future`].
    ///
    /// This is a tt-muncher over the comma-separated argument list: it rewrites each `_` to a
    /// `panic!()` placeholder, passes any other argument through verbatim, and accumulates the
    /// results until the list is exhausted, at which point it emits the assertion. The muncher is
    /// necessary because neither fragment type covers the whole list on its own: `_` is not an
    /// `expr` (ruling out `$(:expr),*`), while explicit arguments may be multi-token expressions
    /// (ruling out `$(:tt),*`). The size spec (`N` or `~ N`) rides along unchanged and selects
    /// which assertion is emitted at the end.
    pub use crate::__assert_async_fn_future_size as assert_async_fn_future_size;

    #[doc(hidden)]
    #[macro_export]
    macro_rules! __assert_async_fn_future_size {
        // All arguments consumed: emit the exact assertion.
        (($f:path) ($w:literal) ($($arg:expr,)*)) => {
            const ACTUAL_BYTES: usize = $crate::size_assert::__macro_refs::size_of_return(
                || $f($($arg),*)
            );
            const EXPECTED_BYTES: usize = ::core::mem::size_of::<[usize; $w]>();

            $crate::size_assert::__macro_refs::FutureHasExpectedWordSize::<
                ACTUAL_BYTES,
                { ACTUAL_BYTES / ::core::mem::size_of::<usize>() },
                EXPECTED_BYTES,
                $w,
            >::assert_size(|| $f($($arg),*));
        };
        // All arguments consumed: emit the approximate assertion.
        (($f:path) (~ $w:literal) ($($arg:expr,)*)) => {
            const ACTUAL_BYTES: usize = $crate::size_assert::__macro_refs::size_of_return(
                || $f($($arg),*)
            );
            const ACTUAL_WORDS: usize = ACTUAL_BYTES / ::core::mem::size_of::<usize>();
            const BOUNDS: (usize, usize) =
                $crate::size_assert::__macro_refs::approx_size_bounds($w);

            $crate::size_assert::__macro_refs::FutureHasApproxWordSize::<
                ACTUAL_BYTES,
                ACTUAL_WORDS,
                $w,
                { BOUNDS.0 },
                { BOUNDS.1 },
                { BOUNDS.0 <= ACTUAL_WORDS && ACTUAL_WORDS <= BOUNDS.1 },
            >::assert_size(|| $f($($arg),*));
        };
        // `_` placeholder (optionally followed by a comma and more arguments).
        (($f:path) ($($size:tt)+) ($($arg:expr,)*) _ $(, $($rest:tt)*)?) => {
            $crate::size_assert::__macro_refs::assert_async_fn_future_size!(
                ($f) ($($size)+) ($($arg,)* ::std::panic!(),) $($($rest)*)?
            );
        };
        // Explicit expression argument (optionally followed by a comma and more arguments).
        (($f:path) ($($size:tt)+) ($($arg:expr,)*) $next:expr $(, $($rest:tt)*)?) => {
            $crate::size_assert::__macro_refs::assert_async_fn_future_size!(
                ($f) ($($size)+) ($($arg,)* $next,) $($($rest)*)?
            );
        };
    }
}

mod __compile_test {
    async fn three_args(_a: u8, _b: u16, _c: u32) -> u64 {
        0
    }

    async fn closure_arg<F: FnOnce(u8) -> u8>(f: F, pad: u64) -> u64 {
        f(0) as u64 + pad
    }

    super::words_of_type!(usize, 1);
    super::same_size!(usize, u64);
    super::words_of_async_fn_future!(three_args, (_, _, _), 1);
    // Mix `_` placeholders with an explicit, multi-token expression argument.
    super::words_of_async_fn_future!(three_args, (_, 1u16 + 1u16, _), 1);
    super::words_of_async_fn_future!(closure_arg, (|x| x, 0u64), 2);
    super::words_of_async_fn_future!(three_args, (_, _, _), ~1);
    super::words_of_expr!(
        three_args(::std::panic!(), ::std::panic!(), ::std::panic!()),
        1
    );
    super::words_of_expr!(
        three_args(::std::panic!(), ::std::panic!(), ::std::panic!()),
        ~1
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn approx_size_bounds() {
        assert_eq!(super::__macro_refs::approx_size_bounds(400), (360, 440));
        assert_eq!(super::__macro_refs::approx_size_bounds(30), (25, 35));
        assert_eq!(super::__macro_refs::approx_size_bounds(2), (0, 7));
    }
}
