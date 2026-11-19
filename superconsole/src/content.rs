/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Provides a variety of utilities for working with [`Line`s](Line).

pub use line::Line;
pub use lines::Lines;
pub use span::Hyperlink;
pub use span::Span;
pub use span::SpanError;

mod line;
mod lines;
mod span;
