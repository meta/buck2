/*
 * Copyright 2018 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

#[inline(always)]
pub(crate) unsafe fn ptr_lifetime<'a, 'b, T: ?Sized>(x: &'a T) -> &'b T {
    unsafe { &*(x as *const T) }
}

/// # Safety
///
/// Same requirements as [`std::mem::transmute`], except that `T` and `U` having the same size is
/// not checked at compile time.
pub(crate) unsafe fn transmute_no_size_check<T, U>(t: T) -> U {
    // SAFETY: The caller guarantees the `transmute` requirements per this function's contract.
    // `ManuallyDrop` transfers ownership of the value to the returned `U` without dropping the `T`.
    unsafe {
        use std::mem::ManuallyDrop;
        let t = ManuallyDrop::new(t);
        std::mem::transmute_copy::<T, U>(&*t)
    }
}

/// `transmute!(from-type, to-type, value)` will do a [`transmute`](std::mem::transmute),
/// but the original and result types must be specified.
macro_rules! transmute {
    ($from:ty, $to:ty, $e:expr) => {
        crate::cast::transmute_no_size_check::<$from, $to>($e)
    };
}

pub(crate) use transmute;
