/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::future::Future;

use buck2_query::query::traversal::ChildVisitor;

use crate::query::graph::node::LabeledNode;

/// Function to return the successors of a node.
pub trait GraphSuccessors<N> {
    fn for_each_successor(&self, node: &N, cb: impl FnMut(&N));
}

pub trait AsyncChildVisitor<N: LabeledNode>: Send + Sync {
    fn for_each_child(
        &self,
        node: &N,
        children: impl ChildVisitor<N>,
    ) -> impl Future<Output = buck2_error::Result<()>> + Send;
}

impl<N: LabeledNode, A: AsyncChildVisitor<N> + ?Sized + Send + Sync> AsyncChildVisitor<N> for &A {
    async fn for_each_child(
        &self,
        node: &N,
        children: impl ChildVisitor<N>,
    ) -> buck2_error::Result<()> {
        (**self).for_each_child(node, children).await
    }
}
