/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

mod builder;
mod critical_path_accessor;
mod graph;
mod potential;
mod types;

#[cfg(test)]
mod test_utils;

pub use builder::GraphBuilder;
pub use builder::PushError;
pub use graph::AddEdgesError;
pub use graph::Graph;
pub use graph::GraphVertex;
pub use graph::TopoSortError;
pub use potential::compute_critical_path_potentials;
pub use types::CriticalPathIndex;
pub use types::CriticalPathVertexData;
pub use types::OptionalVertexId;
pub use types::VertexData;
pub use types::VertexId;
pub use types::VertexKeys;
