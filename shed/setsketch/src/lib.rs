/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Provides the [`SetSketch`] type which implements the setsketch1 algorithm (Ertl 2021,
//! <https://arxiv.org/abs/2101.00314>).

mod sketch;

pub use sketch::SetSketch;
pub use sketch::SetSketchDecodeError;
pub use sketch::SetSketchParams;
pub use sketch::SetSketcher;
