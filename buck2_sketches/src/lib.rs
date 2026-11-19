/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Type-safe wrappers around [`SetSketch`](setsketch::SetSketch) that distinguish between different
//! semantic uses of sketches.

mod typed;

pub use typed::ActionGraphSketch;
pub use typed::ArtifactCountSketch;
pub use typed::ArtifactSizeSketch;
pub use typed::DependencyGraph;
pub use typed::DependencyGraphSketch;
pub use typed::MemoryUsageSketch;
pub use typed::Sketch;
pub use typed::TypedSketch;
