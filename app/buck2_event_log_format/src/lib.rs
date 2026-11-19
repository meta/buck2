/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The encodings of event log entries.
//!
//! The decoders for `BuckEvent` compile to a few functions of several hundred thousand lines of
//! LLVM IR each, which take LLVM the better part of a minute. This crate exists so that they are
//! compiled once, here, early in the build, rather than in `buck2_event_log` and whichever other
//! crates read event logs.

pub mod decode;
pub mod stream_value;
