/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Provides a number of memory-optimized data structures that use high-bit pointer packing tricks,
//! including:
//!
//! - [`MiniVec<T>`]: A `Vec<T>` analogue with single-word stack footprint.
//! - [`MiniBoxSlice<T>`]: A `Box<[T]>` analogue also with a single-word stack footprint.
//! - [`PackedPtr<P>`](crate::packed_ptr::PackedPtr): A generic pointer wrapper that additionally
//!   stores a `u8` of metadata.
//!
//! # Platform requirements
//!
//! This library assumes a 64-bit architecture on which the top 8 bits of userspace pointers are
//! always zeros. As of June 2026, this is true in all standard deployments of x86 and arm,
//! including on the most recent hardware, with the following known exceptions:
//!
//!  1. Intel 5-level page tables. These are usually enabled at the kernel level at least on Linux
//!     these days, however neither Linux nor Windows actually hand out addresses above 2^48 unless
//!     the process asks for it (some linker flag on windows and an mmap hint on Linux).
//!  2. Arm PAC: Widespread particularly on Apple, but generally restricted to pointers other than
//!     normal allocator-returned data pointers (vtables, return addresses, etc.)
//!  3. An ever-growing set of memory safety tooling things on various architectures. These
//!     generally either require build-time opt-ins though (hwasan) or at least per-process
//!     enablement (MTE).
//!
//! Note that this library masks top bits before every access; we do not assume TBI on arm or
//! attempt to use Intel LAM or AMD UAI if available.
//!
//! For any program that does hit one of the exceptions above or for anyone targeting 32 bit
//! platforms, passing `--cfg mini_vec_no_ptr_packing` disables the packing optimization and the
//! metadata reverts to being stored in an adjacent field. That makes this crate fully
//! platform-agnostic, at the cost of stack footprints of all the types doubling. The public API and
//! all observable behavior are otherwise unchanged, so the flag can be flipped without source
//! edits. Cargo builds set it automatically on targets whose pointers are not 64 bits wide; other
//! build systems pass it there.
//!
//! # Nightly Rust
//!
//! This crate builds on stable Rust, but on nightly it additionally enables `dropck_eyepatch`,
//! which relaxes drop-check for [`MiniVec<T>`] the same way `Vec<T>` relaxes it. On stable a
//! `MiniVec` may not be dropped once borrows held in `T` have expired. Cargo builds detect the
//! toolchain automatically; other build systems opt in by passing `--cfg rust_nightly`.

#![deny(missing_docs)]
#![cfg_attr(rust_nightly, feature(dropck_eyepatch))]

mod mini_box_slice;
mod mini_vec;
pub mod packed_ptr;
pub mod size_assert;

pub use mini_box_slice::MiniBoxSlice;
pub use mini_vec::Drain;
pub use mini_vec::IntoIter;
pub use mini_vec::MiniVec;
