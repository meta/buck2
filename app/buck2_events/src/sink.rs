/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Implementations of `[crate::EventSink]` that are useful in different situations. Buck2 primarily uses the `channel`
//! sink during normal operation.
pub(crate) mod channel;
pub mod error_on_event;
pub mod null;
pub mod remote;
#[cfg(fbcode_build)]
pub(crate) mod scribe;
pub(crate) mod smart_truncate_event;
pub mod tee;
